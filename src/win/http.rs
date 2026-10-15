//! Minimal HTTPS client on top of WinHTTP (uses the system proxy and TLS settings).

use windows::Win32::Networking::WinHttp::*;
use windows::core::PCWSTR;

use super::wide;

struct H(*mut core::ffi::c_void);

impl Drop for H {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// Sends a request to `https://{host}{path}` and returns (status code, body).
pub fn request(method: &str, host: &str, path: &str, body: Option<(&str, &[u8])>) -> Result<(u32, Vec<u8>), String> {
    let err = |what: &str| format!("{what} failed: {}", windows::core::Error::from_thread().message());
    unsafe {
        let agent = wide(concat!("Wield/", env!("CARGO_PKG_VERSION")));
        let session = H(WinHttpOpen(PCWSTR(agent.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return Err(err("WinHttpOpen"));
        }
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 20_000);
        let host_w = wide(host);
        let conn = H(WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if conn.0.is_null() {
            return Err(err("Connect"));
        }
        let (verb, path_w) = (wide(method), wide(path));
        let req = H(WinHttpOpenRequest(conn.0, PCWSTR(verb.as_ptr()), PCWSTR(path_w.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE));
        if req.0.is_null() {
            return Err(err("OpenRequest"));
        }
        let headers: Vec<u16>;
        let (hdr, data, len) = match body {
            Some((content_type, bytes)) => {
                headers = format!("Content-Type: {content_type}\r\nAccept: application/json\r\n").encode_utf16().collect();
                (Some(headers.as_slice()), Some(bytes.as_ptr() as *const _), bytes.len() as u32)
            }
            None => {
                headers = "Accept: application/json\r\n".encode_utf16().collect();
                (Some(headers.as_slice()), None, 0)
            }
        };
        WinHttpSendRequest(req.0, hdr, data, len, len, 0).map_err(|e| format!("Request failed: {}", e.message()))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| format!("No response: {}", e.message()))?;

        let mut status = 0u32;
        let mut size = 4u32;
        let mut index = 0u32;
        let _ = WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut _ as *mut _),
            &mut size,
            &mut index,
        );
        let mut out = Vec::new();
        loop {
            let mut avail = 0u32;
            if WinHttpQueryDataAvailable(req.0, &mut avail).is_err() || avail == 0 {
                break;
            }
            let mut buf = vec![0u8; avail as usize];
            let mut read = 0u32;
            if WinHttpReadData(req.0, buf.as_mut_ptr() as *mut _, avail, &mut read).is_err() || read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
            if out.len() > 32 * 1024 * 1024 {
                return Err("Response too large".into());
            }
        }
        Ok((status, out))
    }
}
