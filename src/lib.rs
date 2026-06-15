#![allow(unsafe_op_in_unsafe_fn)]

pub mod portal;
pub mod image_proc;
pub mod pw;

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
use parking_lot::Mutex;
use libc::{c_int, c_uint, c_ulong, c_char};

struct HookState {
    capture: Option<pw::PipewireCapture>,
    portal_session: Option<portal::PortalSession>,
    attempted: bool,
}


static STATE: LazyLock<Mutex<HookState>> = LazyLock::new(|| {
    Mutex::new(HookState {
        capture: None,
        portal_session: None,
        attempted: false,
    })
});


fn get_state() -> &'static Mutex<HookState> {
    &STATE
}

static G_CAPTURE_XIMAGE: AtomicPtr<x11::xlib::XImage> = AtomicPtr::new(std::ptr::null_mut());
static G_INJECTOR_RUNNING: AtomicBool = AtomicBool::new(false);
static G_XSHM_ATTACH_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe fn is_caller_from_meeting_sdk() -> bool {
    let mut callstack: [*mut libc::c_void; 4] = [std::ptr::null_mut(); 4];
    let frames = libc::backtrace(callstack.as_mut_ptr(), 4);
    
    for i in 2..frames as usize {
        let mut info: libc::Dl_info = std::mem::zeroed();
        if libc::dladdr(callstack[i], &mut info) != 0 && !info.dli_fname.is_null() {
            let fname = std::ffi::CStr::from_ptr(info.dli_fname).to_string_lossy();
            if fname.contains("libmeeting_sdk.so") {
                eprintln!("[screenshare-hook] Caller validated: {}", fname);
                return true;
            }
        }
    }
    
    if frames >= 3 {
        let mut info: libc::Dl_info = std::mem::zeroed();
        if libc::dladdr(callstack[2], &mut info) != 0 && !info.dli_fname.is_null() {
            let fname = std::ffi::CStr::from_ptr(info.dli_fname).to_string_lossy();
            eprintln!("[screenshare-hook] XShmAttach from non-meeting caller: {}", fname);
        }
    }
    
    false
}
unsafe fn is_caller_validated() -> bool {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(name) = exe.file_name() {
            if name == "tblive" {
                eprintln!("[screenshare-hook] Caller validated: running inside tblive process");
                return true;
            }
        }
    }
    is_caller_from_meeting_sdk()
}


fn ensure_capture_started(state: &mut HookState) {
    if state.capture.is_some() || state.attempted {
        return;
    }
    state.attempted = true;
    eprintln!("[screenshare-hook] Initiating wayland screenshare capture...");

    match portal::start_screencast() {
        Ok(mut portal_sess) => {
            if portal_sess.streams.is_empty() {
                eprintln!("[screenshare-hook] Portal returned no stream node IDs");
                state.attempted = false;
                return;
            }
            let stream_info = &portal_sess.streams[0];
            let node_id = stream_info.node_id;
            let target_object = stream_info.pipewire_serial.map(|s| s.to_string());
            
            let owned_fd = match portal_sess.take_fd() {
                Some(fd) => fd,
                None => {
                    eprintln!("[screenshare-hook] PipeWire FD already consumed");
                    state.attempted = false;
                    return;
                }
            };

            match pw::start_capture(owned_fd, node_id, target_object) {
                Ok(cap) => {
                    state.capture = Some(cap);
                    state.portal_session = Some(portal_sess); // Keep the portal session alive!
                    eprintln!("[screenshare-hook] Screen capture started successfully via PipeWire node {}", node_id);
                }
                Err(e) => {
                    eprintln!("[screenshare-hook] Failed to start PipeWire capture: {}", e);
                    state.attempted = false;
                }
            }
        }
        Err(e) => {
            eprintln!("[screenshare-hook] XDG Portal handshake failed: {}", e);
            state.attempted = false;
        }
    }
}


fn start_portal_session() {
    let mut guard = get_state().lock();
    ensure_capture_started(&mut guard);
    if let Some(ref cap) = guard.capture {
        let frame_handle = cap.latest_frame_handle();
        G_INJECTOR_RUNNING.store(true, Ordering::Release);
        thread::spawn(move || {
            injector_loop(frame_handle);
        });
    }
}



fn injector_loop(latest_frame: std::sync::Arc<Mutex<Option<pw::FrameData>>>) {

    eprintln!("[screenshare-hook] injector thread started");
    let mut inject_count: u64 = 0;
    
    let mut ximage: *mut x11::xlib::XImage = std::ptr::null_mut();
    while G_INJECTOR_RUNNING.load(Ordering::Acquire) {
        ximage = G_CAPTURE_XIMAGE.load(Ordering::Acquire);
        if !ximage.is_null() {
            unsafe {
                let img = &*ximage;
                if !img.data.is_null() && img.width >= 640 && img.height >= 480 && img.width < 16384 && img.height < 16384 {
                    break;
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    
    if !G_INJECTOR_RUNNING.load(Ordering::Acquire) {
        return;
    }
    
    let (width, height, bytes_per_line, ximage_data) = unsafe {
        let img = &*ximage;
        (img.width, img.height, img.bytes_per_line as usize, img.data as *mut u8)
    };
    
    let buf_size = bytes_per_line * (height as usize);
    let mut staging_buffer = vec![0u8; buf_size];
    
    eprintln!("[screenshare-hook] injector locked XImage: {}x{} data={:?}", width, height, ximage_data);
    
    while G_INJECTOR_RUNNING.load(Ordering::Acquire) {
        let mut frame_copied = false;
        {
            let guard = latest_frame.lock();
            if let Some(frame) = &*guard {
                if let Err(e) = image_proc::resize_and_convert(
                    &frame.data,
                    frame.width,
                    frame.height,
                    &mut staging_buffer,
                    width as u32,
                    height as u32,
                ) {
                    eprintln!("[screenshare-hook] resize_and_convert failed in injector: {}", e);
                } else {
                    frame_copied = true;
                }
            }
        }

        
        if frame_copied && G_INJECTOR_RUNNING.load(Ordering::Acquire) {
            unsafe {
                std::ptr::copy_nonoverlapping(staging_buffer.as_ptr(), ximage_data, buf_size);
            }
            inject_count += 1;
            if inject_count == 1 || inject_count % 200 == 0 {
                eprintln!("[screenshare-hook] injector frame #{}", inject_count);
            }
        }
        
        thread::sleep(Duration::from_millis(33));
    }
    
    eprintln!("[screenshare-hook] injector thread stopped");
    
    let mut guard = get_state().lock();
    if let Some(mut cap) = guard.capture.take() {
        cap.stop();
    }
    guard.portal_session = None; // Terminate portal session and Close it
    guard.attempted = false;
    G_CAPTURE_XIMAGE.store(std::ptr::null_mut(), Ordering::Release);

}

type XShmAttachType = unsafe extern "C" fn(
    *mut x11::xlib::Display,
    *mut x11::xshm::XShmSegmentInfo,
) -> c_int;

type XShmDetachType = unsafe extern "C" fn(
    *mut x11::xlib::Display,
    *mut x11::xshm::XShmSegmentInfo,
) -> c_int;

type XShmGetImageType = unsafe extern "C" fn(
    *mut x11::xlib::Display,
    x11::xlib::Drawable,
    *mut x11::xlib::XImage,
    c_int,
    c_int,
    c_ulong,
) -> c_int;

type XShmCreateImageType = unsafe extern "C" fn(
    *mut x11::xlib::Display,
    *mut x11::xlib::Visual,
    c_uint,
    c_int,
    *mut c_char,
    *mut x11::xshm::XShmSegmentInfo,
    c_uint,
    c_uint,
) -> *mut x11::xlib::XImage;

struct DlHandle(*mut libc::c_void);
unsafe impl Send for DlHandle {}
unsafe impl Sync for DlHandle {}

static XEXT_HANDLE: LazyLock<DlHandle> = LazyLock::new(|| {
    unsafe {
        let mut handle = libc::dlopen(b"libXext.so.6\0".as_ptr() as *const c_char, libc::RTLD_LAZY);
        if handle.is_null() {
            handle = libc::dlopen(b"libXext.so\0".as_ptr() as *const c_char, libc::RTLD_LAZY);
        }
        if handle.is_null() {
            eprintln!("[screenshare-hook] Failed to dlopen libXext");
        }
        DlHandle(handle)
    }
});

static X11_HANDLE: LazyLock<DlHandle> = LazyLock::new(|| {
    unsafe {
        let mut handle = libc::dlopen(b"libX11.so.6\0".as_ptr() as *const c_char, libc::RTLD_LAZY);
        if handle.is_null() {
            handle = libc::dlopen(b"libX11.so\0".as_ptr() as *const c_char, libc::RTLD_LAZY);
        }
        if handle.is_null() {
            eprintln!("[screenshare-hook] Failed to dlopen libX11");
        }
        DlHandle(handle)
    }
});

static ORIGINAL_XSHMATTACH: LazyLock<Option<XShmAttachType>> = LazyLock::new(|| {
    unsafe {
        let handle = XEXT_HANDLE.0;
        if handle.is_null() { return None; }

        let symbol = libc::dlsym(handle, b"XShmAttach\0".as_ptr() as *const c_char);
        if symbol.is_null() { None } else { Some(std::mem::transmute(symbol)) }
    }
});

static ORIGINAL_XSHMDETACH: LazyLock<Option<XShmDetachType>> = LazyLock::new(|| {
    unsafe {
        let handle = XEXT_HANDLE.0;
        if handle.is_null() { return None; }

        let symbol = libc::dlsym(handle, b"XShmDetach\0".as_ptr() as *const c_char);
        if symbol.is_null() { None } else { Some(std::mem::transmute(symbol)) }
    }
});

static ORIGINAL_XSHMGETIMAGE: LazyLock<Option<XShmGetImageType>> = LazyLock::new(|| {
    unsafe {
        let handle = XEXT_HANDLE.0;
        if handle.is_null() { return None; }

        let symbol = libc::dlsym(handle, b"XShmGetImage\0".as_ptr() as *const c_char);
        if symbol.is_null() { None } else { Some(std::mem::transmute(symbol)) }
    }
});

static ORIGINAL_XSHMCREATEIMAGE: LazyLock<Option<XShmCreateImageType>> = LazyLock::new(|| {
    unsafe {
        let handle = XEXT_HANDLE.0;
        if handle.is_null() { return None; }

        let symbol = libc::dlsym(handle, b"XShmCreateImage\0".as_ptr() as *const c_char);
        if symbol.is_null() { None } else { Some(std::mem::transmute(symbol)) }
    }
});


#[unsafe(no_mangle)]
pub unsafe extern "C" fn XShmCreateImage(
    dpy: *mut x11::xlib::Display,
    visual: *mut x11::xlib::Visual,
    depth: c_uint,
    format: c_int,
    data: *mut c_char,
    shminfo: *mut x11::xshm::XShmSegmentInfo,
    width: c_uint,
    height: c_uint,
) -> *mut x11::xlib::XImage {
    let original = match &*ORIGINAL_XSHMCREATEIMAGE {
        Some(f) => f,
        None => return std::ptr::null_mut(),
    };
    
    let image = original(dpy, visual, depth, format, data, shminfo, width, height);
    
    let attach_count = G_XSHM_ATTACH_COUNT.load(Ordering::Acquire);
    if !image.is_null() && width >= 640 && height >= 480 && attach_count >= 1 {
        eprintln!(
            "[screenshare-hook] XShmCreateImage ({}x{}, attach={})",
            width, height, attach_count
        );
        G_CAPTURE_XIMAGE.store(image, Ordering::Release);
    }
    
    image
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn XShmAttach(
    dpy: *mut x11::xlib::Display,
    shminfo: *mut x11::xshm::XShmSegmentInfo,
) -> c_int {
    let original = match &*ORIGINAL_XSHMATTACH {
        Some(f) => f,
        None => return 0,
    };
    
    let result = original(dpy, shminfo);
    
    if is_caller_validated() {
        let count = G_XSHM_ATTACH_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        eprintln!("[screenshare-hook] XShmAttach from meeting SDK (count={})", count);
        
        if count == 2 {
            thread::spawn(|| {
                start_portal_session();
            });
        }
    }
    
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn XShmDetach(
    dpy: *mut x11::xlib::Display,
    shminfo: *mut x11::xshm::XShmSegmentInfo,
) -> c_int {
    G_INJECTOR_RUNNING.store(false, Ordering::Release);
    
    let original = match &*ORIGINAL_XSHMDETACH {
        Some(f) => f,
        None => return 0,
    };
    
    original(dpy, shminfo)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn XShmGetImage(
    dpy: *mut x11::xlib::Display,
    d: x11::xlib::Drawable,
    image: *mut x11::xlib::XImage,
    x: c_int,
    y: c_int,
    plane_mask: c_ulong,
) -> c_int {
    let original = match &*ORIGINAL_XSHMGETIMAGE {
        Some(f) => f,
        None => return 0,
    };
    let result = original(dpy, d, image, x, y, plane_mask);

    if !G_INJECTOR_RUNNING.load(Ordering::Acquire) && !image.is_null() {
        let mut guard = get_state().lock();
        ensure_capture_started(&mut guard);

        if let Some(cap) = &guard.capture {
            if let Some(frame) = cap.get_latest_frame() {
                let dst_slice = std::slice::from_raw_parts_mut(
                    (*image).data as *mut u8,
                    ((*image).width as usize) * ((*image).height as usize) * 4,
                );

                let _ = image_proc::resize_and_convert(
                    &frame.data,
                    frame.width,
                    frame.height,
                    dst_slice,
                    (*image).width as u32,
                    (*image).height as u32,
                );
            }
        }
    }

    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn XGetImage(
    dpy: *mut x11::xlib::Display,
    d: x11::xlib::Drawable,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
    plane_mask: c_ulong,
    format: c_int,
) -> *mut x11::xlib::XImage {
    static ORIGINAL_XGETIMAGE: LazyLock<Option<unsafe extern "C" fn(
        *mut x11::xlib::Display,
        x11::xlib::Drawable,
        c_int,
        c_int,
        c_uint,
        c_uint,
        c_ulong,
        c_int,
    ) -> *mut x11::xlib::XImage>> = LazyLock::new(|| {
        unsafe {
            let handle = X11_HANDLE.0;
            if handle.is_null() { return None; }

            let symbol = libc::dlsym(handle, b"XGetImage\0".as_ptr() as *const c_char);
            if symbol.is_null() { None } else { Some(std::mem::transmute(symbol)) }
        }
    });


    let original = match &*ORIGINAL_XGETIMAGE {
        Some(f) => f,
        None => return std::ptr::null_mut(),
    };
    let image = original(dpy, d, x, y, width, height, plane_mask, format);

    if !G_INJECTOR_RUNNING.load(Ordering::Acquire) && !image.is_null() {
        let mut guard = get_state().lock();
        ensure_capture_started(&mut guard);

        if let Some(cap) = &guard.capture {
            if let Some(frame) = cap.get_latest_frame() {
                let dst_slice = std::slice::from_raw_parts_mut(
                    (*image).data as *mut u8,
                    (width as usize) * (height as usize) * 4,
                );

                let _ = image_proc::resize_and_convert(
                    &frame.data,
                    frame.width,
                    frame.height,
                    dst_slice,
                    width,
                    height,
                );
            }
        }
    }

    image
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn shmdt(shmaddr: *const libc::c_void) -> c_int {
    G_INJECTOR_RUNNING.store(false, Ordering::Release);
    
    static ORIGINAL_SHMDT: LazyLock<Option<unsafe extern "C" fn(*const libc::c_void) -> c_int>> = LazyLock::new(|| {
        unsafe {
            let mut handle = libc::dlopen(b"libc.so.6\0".as_ptr() as *const c_char, libc::RTLD_LAZY);
            if handle.is_null() {
                handle = libc::dlopen(b"libc.so\0".as_ptr() as *const c_char, libc::RTLD_LAZY);
            }
            if handle.is_null() { return None; }
            let symbol = libc::dlsym(handle, b"shmdt\0".as_ptr() as *const c_char);
            if symbol.is_null() { None } else { Some(std::mem::transmute(symbol)) }
        }
    });


    if let Some(f) = &*ORIGINAL_SHMDT {
        f(shmaddr)
    } else {
        -1
    }
}

