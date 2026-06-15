use pipewire as pw;
use pw::spa;
use spa::pod::Pod;
use parking_lot::Mutex;
use std::sync::Arc;
use std::os::fd::OwnedFd;
use std::thread;

unsafe extern "C" {
    fn pw_main_loop_quit(loop_: *mut std::ffi::c_void);
}

#[derive(Clone, Debug)]
pub struct FrameData {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub data: Vec<u8>,
}

pub struct PipewireCapture {
    latest_frame: Arc<Mutex<Option<FrameData>>>,
    mainloop_raw: *mut std::ffi::c_void,
    thread_handle: Option<thread::JoinHandle<()>>,
}

unsafe impl Send for PipewireCapture {}
unsafe impl Sync for PipewireCapture {}

impl PipewireCapture {
    pub fn get_latest_frame(&self) -> Option<FrameData> {
        self.latest_frame.lock().clone()
    }

    pub fn latest_frame_handle(&self) -> Arc<Mutex<Option<FrameData>>> {
        self.latest_frame.clone()
    }

    pub fn stop(&mut self) {
        if !self.mainloop_raw.is_null() {
            unsafe {
                pw_main_loop_quit(self.mainloop_raw);
            }
            self.mainloop_raw = std::ptr::null_mut();
        }

        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PipewireCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

struct StreamData {
    format: spa::param::video::VideoInfoRaw,
    latest_frame: Arc<Mutex<Option<FrameData>>>,
}

static FRAME_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static NONE_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, Copy)]
struct ParamBuffers(u32);
impl ParamBuffers {
    const BUFFERS: Self = Self(1); // SPA_PARAM_BUFFERS_buffers
    const BLOCKS: Self = Self(2);  // SPA_PARAM_BUFFERS_blocks
    const SIZE: Self = Self(3);    // SPA_PARAM_BUFFERS_size
    const STRIDE: Self = Self(4);  // SPA_PARAM_BUFFERS_stride
    const DATA_TYPE: Self = Self(6); // SPA_PARAM_BUFFERS_dataType
    
    pub fn as_raw(&self) -> u32 {
        self.0
    }
}



fn serialize_object(obj: spa::pod::Object) -> Result<Vec<u8>, String> {
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    ).map_err(|e| e.to_string()).map(|s| s.0.into_inner())
}

fn build_buffers_param(format: &spa::param::video::VideoInfoRaw) -> Result<Vec<u8>, String> {
    let size = format.size();
    let stride = (size.width as i32).saturating_mul(4);
    let block_size = stride.saturating_mul(size.height as i32);
    
    // We request MemPtr (1) and MemFd (2) data types
    let memptr = 1i32 << 1; // 1 << SPA_DATA_MemPtr
    let memfd = 1i32 << 2;  // 1 << SPA_DATA_MemFd
    let data_types = memptr | memfd;

    // Construct properties manually to bypass libspa Choice, Range, Flags macro bugs
    let prop_buffers = spa::pod::property!(
        ParamBuffers::BUFFERS,
        spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(
            spa::utils::Choice::<i32>(
                spa::utils::ChoiceFlags::empty(),
                spa::utils::ChoiceEnum::<i32>::Range {
                    default: 4,
                    min: 2,
                    max: 16,
                }
            )
        ))
    );

    let prop_blocks = spa::pod::property!(
        ParamBuffers::BLOCKS,
        spa::pod::Value::Int(1)
    );

    let prop_size = spa::pod::property!(
        ParamBuffers::SIZE,
        spa::pod::Value::Int(block_size)
    );

    let prop_stride = spa::pod::property!(
        ParamBuffers::STRIDE,
        spa::pod::Value::Int(stride)
    );

    let prop_data_type = spa::pod::property!(
        ParamBuffers::DATA_TYPE,
        spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(
            spa::utils::Choice::<i32>(
                spa::utils::ChoiceFlags::empty(),
                spa::utils::ChoiceEnum::<i32>::Flags {
                    default: data_types,
                    flags: vec![memptr, memfd],
                }
            )
        ))
    );

    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            prop_buffers,
            prop_blocks,
            prop_size,
            prop_stride,
            prop_data_type,
        ],
    };

    serialize_object(obj)
}

pub fn start_capture(
    fd: OwnedFd,
    node_id: u32,
    target_object: Option<String>,
) -> Result<PipewireCapture, Box<dyn std::error::Error>> {
    pw::init();

    let latest_frame = Arc::new(Mutex::new(None));
    let latest_frame_thread = latest_frame.clone();

    let (tx_started, rx_started) = std::sync::mpsc::channel();
    let tx_started_clone = tx_started.clone();

    let thread_handle = thread::spawn(move || {
        let run_loop = move || -> Result<(), String> {
            let mainloop = pw::main_loop::MainLoopBox::new(None).map_err(|e| e.to_string())?;
            let context = pw::context::ContextBox::new(mainloop.loop_(), None).map_err(|e| e.to_string())?;

            let core = context.connect_fd(fd, None).map_err(|e| e.to_string())?;

            let stream_data = StreamData {
                format: spa::param::video::VideoInfoRaw::default(),
                latest_frame: latest_frame_thread,
            };

            let mut props = pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            };
            if let Some(target) = target_object.as_deref() {
                props.insert("target.object", target);
            }

            let stream = pw::stream::StreamBox::new(
                &core,
                "dingtalk-screencast",
                props,
            ).map_err(|e| e.to_string())?;

            let _listener = stream
                .add_local_listener_with_user_data(stream_data)
                .state_changed(|_, _, state, error| {
                    eprintln!("[screenshare-hook] pw stream state changed to {:?} (error: {:?})", state, error);
                })
                .param_changed(|stream, data, id, param| {
                    let Some(param) = param else {
                        return;
                    };

                    if id != spa::param::ParamType::Format.as_raw() {
                        return;
                    }

                    let Ok((media_type, media_subtype)) =
                        spa::param::format_utils::parse_format(param)
                    else {
                        return;
                    };

                    if media_type != spa::param::format::MediaType::Video
                        || media_subtype != spa::param::format::MediaSubtype::Raw
                    {
                        return;
                    }

                    if let Err(e) = data.format.parse(param) {
                        eprintln!("[screenshare-hook] failed to parse raw video format: {:?}", e);
                        return;
                    }

                    // Complete buffer negotiation
                    let Ok(values) = build_buffers_param(&data.format) else {
                        return;
                    };
                    let Some(pod) = Pod::from_bytes(&values) else {
                        return;
                    };
                    let mut params = [pod];
                    if let Err(e) = stream.update_params(&mut params) {
                        eprintln!("[screenshare-hook] pw update_params(Buffers) failed: {:?}", e);
                    }
                })
                .process(|stream, data| {
                    let Some(mut buffer) = stream.dequeue_buffer() else {
                        return;
                    };

                    let datas = buffer.datas_mut();
                    if datas.is_empty() {
                        eprintln!("[screenshare-hook] pw process: buffer data is empty");
                        return;
                    }

                    let d = &mut datas[0];
                    let chunk = d.chunk();
                    let chunk_size = chunk.size() as usize;
                    let chunk_offset = chunk.offset() as usize;
                    let chunk_stride = chunk.stride() as u32;

                    match d.data() {
                        Some(bytes) => {
                            let start = chunk_offset.min(bytes.len());
                            let end = (chunk_offset + chunk_size).min(bytes.len());
                            let frame_bytes = &bytes[start..end];

                            let size = data.format.size();
                            let stride = if chunk_stride == 0 { size.width * 4 } else { chunk_stride };

                            let count = FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            if count == 1 || count % 200 == 0 {
                                eprintln!("[screenshare-hook] pw process: frame #{} size={}x{} stride={}", count, size.width, size.height, stride);
                            }

                            let mut guard = data.latest_frame.lock();
                            if let Some(ref mut existing) = *guard {
                                existing.width = size.width;
                                existing.height = size.height;
                                existing.stride = stride;
                                if existing.data.len() != frame_bytes.len() {
                                    existing.data.resize(frame_bytes.len(), 0);
                                }
                                existing.data.copy_from_slice(frame_bytes);
                            } else {
                                *guard = Some(FrameData {
                                    width: size.width,
                                    height: size.height,
                                    stride,
                                    data: frame_bytes.to_vec(),
                                });
                            }
                        }
                        None => {
                            let count = NONE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            if count == 1 || count % 200 == 0 {
                                eprintln!("[screenshare-hook] pw process: d.data() is None (memfd/dmabuf?) #{}", count);
                            }
                        }
                    }
                })
                .register().map_err(|e| e.to_string())?;

            // Advertise accepted video formats (restrict to RGBx / RGBA for color swap)
            let obj = spa::pod::object!(
                spa::utils::SpaTypes::ObjectParamFormat,
                spa::param::ParamType::EnumFormat,
                spa::pod::property!(
                    spa::param::format::FormatProperties::MediaType,
                    Id,
                    spa::param::format::MediaType::Video
                ),
                spa::pod::property!(
                    spa::param::format::FormatProperties::MediaSubtype,
                    Id,
                    spa::param::format::MediaSubtype::Raw
                ),
                spa::pod::property!(
                    spa::param::format::FormatProperties::VideoFormat,
                    Choice,
                    Enum,
                    Id,
                    spa::param::video::VideoFormat::BGRx, // Preferred for KDE
                    spa::param::video::VideoFormat::BGRx,
                    spa::param::video::VideoFormat::RGBx,
                    spa::param::video::VideoFormat::BGRA,
                    spa::param::video::VideoFormat::RGBA,
                ),
                spa::pod::property!(
                    spa::param::format::FormatProperties::VideoSize,
                    Choice,
                    Range,
                    Rectangle,
                    spa::utils::Rectangle {
                        width: 1280,
                        height: 720,
                    },
                    spa::utils::Rectangle {
                        width: 1,
                        height: 1,
                    },
                    spa::utils::Rectangle {
                        width: 4096,
                        height: 4096,
                    }
                ),
                spa::pod::property!(
                    spa::param::format::FormatProperties::VideoFramerate,
                    Choice,
                    Range,
                    Fraction,
                    spa::utils::Fraction { num: 30, denom: 1 },
                    spa::utils::Fraction { num: 0, denom: 1 },
                    spa::utils::Fraction {
                        num: 1000,
                        denom: 1,
                    }
                ),
            );

            let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
                std::io::Cursor::new(Vec::new()),
                &spa::pod::Value::Object(obj),
            ).map_err(|e| e.to_string())?
            .0
            .into_inner();

            let mut params = [Pod::from_bytes(&values).ok_or_else(|| "failed to parse Pod".to_string())?];

            let target_id = if target_object.is_some() { None } else { Some(node_id) };
            stream.connect(
                spa::utils::Direction::Input,
                target_id,
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut params,
            ).map_err(|e| e.to_string())?;

            let ptr_val = mainloop.as_raw_ptr() as usize;
            tx_started_clone.send(Ok(ptr_val)).map_err(|_| "Failed to send mainloop pointer".to_string())?;

            mainloop.run();

            Ok(())
        };

        if let Err(e) = run_loop() {
            let _ = tx_started.send(Err(e));
        }
    });

    match rx_started.recv() {
        Ok(Ok(mainloop_raw_val)) => {
            Ok(PipewireCapture {
                latest_frame,
                mainloop_raw: mainloop_raw_val as *mut std::ffi::c_void,
                thread_handle: Some(thread_handle),
            })
        }
        Ok(Err(e)) => Err(Box::<dyn std::error::Error>::from(e)),
        Err(_) => Err("PipeWire background thread failed during initialization".into()),
    }
}
