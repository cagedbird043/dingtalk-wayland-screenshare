use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedValue, Value, OwnedObjectPath, ObjectPath};

fn rand_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| std::time::Duration::from_secs(0))
        .as_nanos();
    format!("{:x}", nanos)
}

fn get_sender_name(conn: &Connection) -> String {
    let name = conn.unique_name()
        .map(|n| n.as_str())
        .unwrap_or("");
    let name = name.strip_prefix(':').unwrap_or(name);
    name.replace('.', "_")
}

fn str_to_owned_value(s: &str) -> Result<OwnedValue, Box<dyn std::error::Error>> {
    Ok(Value::from(s).try_to_owned()?)
}

fn u32_to_owned_value(n: u32) -> Result<OwnedValue, Box<dyn std::error::Error>> {
    Ok(Value::from(n).try_to_owned()?)
}

fn bool_to_owned_value(b: bool) -> Result<OwnedValue, Box<dyn std::error::Error>> {
    Ok(Value::from(b).try_to_owned()?)
}

fn extract_string(val: &OwnedValue) -> Result<String, Box<dyn std::error::Error>> {
    if let Ok(s) = Value::from(val.try_clone()?).downcast::<String>() {
        return Ok(s);
    }
    if let Ok(op) = Value::from(val.try_clone()?).downcast::<ObjectPath<'static>>() {
        return Ok(op.to_string());
    }
    if let Ok(op) = Value::from(val.try_clone()?).downcast::<OwnedObjectPath>() {
        return Ok(op.to_string());
    }
    Err("Failed to extract string from variant".into())
}

#[derive(Debug, Clone)]
pub struct PortalStream {
    pub node_id: u32,
    pub pipewire_serial: Option<u64>,
}

pub struct PortalSession {
    conn: Connection,
    session_handle: OwnedObjectPath,
    fd: Option<std::os::fd::OwnedFd>,
    pub streams: Vec<PortalStream>,
}

impl PortalSession {
    pub fn take_fd(&mut self) -> Option<std::os::fd::OwnedFd> {
        self.fd.take()
    }
}

impl Drop for PortalSession {
    fn drop(&mut self) {
        let path = self.session_handle.to_string();
        eprintln!("[screenshare-hook] closing portal session: {}", path);
        if let Ok(proxy) = Proxy::new(
            &self.conn,
            "org.freedesktop.portal.Desktop",
            path.as_str(),
            "org.freedesktop.portal.Session",
        ) {
            let _: Result<(), _> = proxy.call("Close", &());
        }
    }
}

pub fn start_screencast() -> Result<PortalSession, Box<dyn std::error::Error>> {
    let conn = Connection::session()?;
    let sender_name = get_sender_name(&conn);
    
    // Create the ScreenCast portal proxy
    let portal_proxy = Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.ScreenCast",
    )?;

    // 1. CreateSession
    let req_token_1 = format!("req_{}", rand_id());
    let req_path_1 = format!("/org/freedesktop/portal/desktop/request/{}/{}", sender_name, req_token_1);
    
    let req_proxy_1 = Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        req_path_1.as_str(),
        "org.freedesktop.portal.Request",
    )?;
    let mut signals_1 = req_proxy_1.receive_signal("Response")?;

    let session_token = format!("sess_{}", rand_id());
    let mut options = HashMap::new();
    options.insert("session_handle_token".to_string(), str_to_owned_value(&session_token)?);
    options.insert("handle_token".to_string(), str_to_owned_value(&req_token_1)?);

    let _req_handle: OwnedObjectPath = portal_proxy.call("CreateSession", &(options,))?;

    let msg_1 = signals_1.next().ok_or("No response for CreateSession")?;
    let (response_code_1, results_1): (u32, HashMap<String, OwnedValue>) = msg_1.body().deserialize()?;
    if response_code_1 != 0 {
        return Err(format!("CreateSession failed with response code {}", response_code_1).into());
    }
    let session_handle_val = results_1.get("session_handle").ok_or("Missing session_handle in CreateSession response")?;
    let session_handle_str = extract_string(session_handle_val)?;
    let session_handle = OwnedObjectPath::try_from(session_handle_str)?;

    // 2. SelectSources
    let req_token_2 = format!("req_{}", rand_id());
    let req_path_2 = format!("/org/freedesktop/portal/desktop/request/{}/{}", sender_name, req_token_2);
    
    let req_proxy_2 = Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        req_path_2.as_str(),
        "org.freedesktop.portal.Request",
    )?;
    let mut signals_2 = req_proxy_2.receive_signal("Response")?;

    let mut options = HashMap::new();
    options.insert("handle_token".to_string(), str_to_owned_value(&req_token_2)?);
    options.insert("types".to_string(), u32_to_owned_value(3u32)?); // MONITOR (1) | WINDOW (2)
    options.insert("multiple".to_string(), bool_to_owned_value(false)?);
    options.insert("cursor_mode".to_string(), u32_to_owned_value(2u32)?); // Embedded

    let _req_handle: OwnedObjectPath = portal_proxy.call("SelectSources", &(&session_handle, options))?;

    let msg_2 = signals_2.next().ok_or("No response for SelectSources")?;
    let (response_code_2, _results_2): (u32, HashMap<String, OwnedValue>) = msg_2.body().deserialize()?;
    if response_code_2 != 0 {
        return Err(format!("SelectSources failed with response code {}", response_code_2).into());
    }

    // 3. Start
    let req_token_3 = format!("req_{}", rand_id());
    let req_path_3 = format!("/org/freedesktop/portal/desktop/request/{}/{}", sender_name, req_token_3);
    
    let req_proxy_3 = Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        req_path_3.as_str(),
        "org.freedesktop.portal.Request",
    )?;
    let mut signals_3 = req_proxy_3.receive_signal("Response")?;

    let mut options = HashMap::new();
    options.insert("handle_token".to_string(), str_to_owned_value(&req_token_3)?);

    let parent_window = "";
    let _req_handle: OwnedObjectPath = portal_proxy.call("Start", &(&session_handle, parent_window, options))?;

    let msg_3 = signals_3.next().ok_or("No response for Start")?;
    let (response_code_3, results_3): (u32, HashMap<String, OwnedValue>) = msg_3.body().deserialize()?;
    if response_code_3 != 0 {
        return Err(format!("Start failed with response code {}", response_code_3).into());
    }

    let streams_val = results_3.get("streams").ok_or("Missing streams in Start response")?;
    let streams_value = Value::from(streams_val.try_clone()?);
    let streams = streams_value.downcast::<Vec<(u32, HashMap<String, OwnedValue>)>>()?;

    // 4. OpenPipeWireRemote
    let options: HashMap<String, OwnedValue> = HashMap::new();
    let fd_val: zbus::zvariant::OwnedFd = portal_proxy.call("OpenPipeWireRemote", &(&session_handle, options))?;
    
    let std_fd: std::os::fd::OwnedFd = fd_val.into();

    let mut portal_streams = Vec::new();
    for (node_id, props) in streams {
        let mut pipewire_serial = None;
        if let Some(serial_val) = props.get("pipewire-serial") {
            if let Ok(val) = Value::from(serial_val.try_clone()?).downcast::<u64>() {
                pipewire_serial = Some(val);
            }
        }
        portal_streams.push(PortalStream {
            node_id,
            pipewire_serial,
        });
    }

    Ok(PortalSession {
        conn,
        session_handle,
        fd: Some(std_fd),
        streams: portal_streams,
    })
}
