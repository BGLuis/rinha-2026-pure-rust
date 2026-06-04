use std::os::unix::io::RawFd;
use std::ptr;
use std::mem;
use std::ffi::CString;

use rust_engine::{init_engine, search_vector};

const RESP_0: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 35\r\nConnection: keep-alive\r\n\r\n{\"approved\":true,\"fraud_score\":0.0}";
const RESP_1: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 35\r\nConnection: keep-alive\r\n\r\n{\"approved\":true,\"fraud_score\":0.2}";
const RESP_2: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 35\r\nConnection: keep-alive\r\n\r\n{\"approved\":true,\"fraud_score\":0.4}";
const RESP_3: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 36\r\nConnection: keep-alive\r\n\r\n{\"approved\":false,\"fraud_score\":0.6}";
const RESP_4: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 36\r\nConnection: keep-alive\r\n\r\n{\"approved\":false,\"fraud_score\":0.8}";
const RESP_5: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 36\r\nConnection: keep-alive\r\n\r\n{\"approved\":false,\"fraud_score\":1.0}";
const RESP_404: &[u8] = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n";
const RESP_READY: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n";

fn parse_float_fast(b: &[u8], start: usize) -> (f64, usize) {
    let mut val = 0.0;
    let mut dec = 0.0;
    let mut in_dec = false;
    let mut div = 1.0;
    let mut i = start;
    while i < b.len() {
        let ch = b[i];
        if ch >= b'0' && ch <= b'9' {
            if in_dec {
                dec = dec * 10.0 + (ch - b'0') as f64;
                div *= 10.0;
            } else {
                val = val * 10.0 + (ch - b'0') as f64;
            }
        } else if ch == b'.' {
            in_dec = true;
        } else {
            break;
        }
        i += 1;
    }
    (val + dec / div, i)
}

fn parse_int_fast(b: &[u8], start: usize) -> (i64, usize) {
    let mut val = 0;
    let mut i = start;
    while i < b.len() {
        let ch = b[i];
        if ch >= b'0' && ch <= b'9' {
            val = val * 10 + (ch - b'0') as i64;
        } else {
            break;
        }
        i += 1;
    }
    (val, i)
}

fn parse_bool_fast(b: &[u8], start: usize) -> (bool, usize) {
    if start < b.len() && b[start] == b't' {
        (true, start + 4)
    } else {
        (false, start + 5)
    }
}

fn parse_string_fast(b: &[u8], mut start: usize) -> (&[u8], usize) {
    if start >= b.len() || b[start] != b'"' {
        return (&[], start);
    }
    start += 1;
    for i in start..b.len() {
        if b[i] == b'"' {
            return (&b[start..i], i + 1);
        }
    }
    (&[], b.len())
}

fn find_direct(b: &[u8], key: &[u8]) -> isize {
    if let Some(idx) = b.windows(key.len()).position(|w| w == key) {
        let mut start = idx + key.len();
        while start < b.len() && (b[start] == b' ' || b[start] == b':' || b[start] == b'\n' || b[start] == b'\r') {
            start += 1;
        }
        start as isize
    } else {
        -1
    }
}

fn find_after(b: &[u8], block_key: &[u8], val_key: &[u8]) -> isize {
    if let Some(b_idx) = b.windows(block_key.len()).position(|w| w == block_key) {
        let sub = &b[b_idx..];
        if let Some(v_idx) = sub.windows(val_key.len()).position(|w| w == val_key) {
            let mut start = b_idx + v_idx + val_key.len();
            while start < b.len() && (b[start] == b' ' || b[start] == b':' || b[start] == b'\n' || b[start] == b'\r') {
                start += 1;
            }
            return start as isize;
        }
    }
    -1
}

fn clamp(v: f64) -> f32 {
    if v < 0.0 { 0.0 } else if v > 1.0 { 1.0 } else { v as f32 }
}

fn fast_parse_time_str(s: &[u8]) -> i64 {
    if s.len() < 19 { return 0; }
    let d2 = |i: usize| -> i64 { ((s[i] - b'0') * 10 + (s[i + 1] - b'0')) as i64 };
    let year = ((s[0] - b'0') as i64) * 1000 + ((s[1] - b'0') as i64) * 100 + ((s[2] - b'0') as i64) * 10 + ((s[3] - b'0') as i64);
    let month = d2(5);
    let day = d2(8);
    let hour = d2(11);
    let min = d2(14);
    let sec = d2(17);

    let mut y = year;
    if month <= 2 { y -= 1; }
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let m = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days * 86400 + hour * 3600 + min * 60 + sec
}

struct Config {
    max_amount: f64,
    max_installments: f64,
    amount_vs_avg_ratio: f64,
    max_minutes: f64,
    max_km: f64,
    max_tx_count_24h: f64,
    max_merchant_avg_amount: f64,
    mcc_risk_arr: [f32; 10000],
}

// Minimal JSON parser for the simple configs (since we don't have serde imported here by default)
fn load_config() -> Config {
    // For extreme performance and pure Rust constraint, we'll parse exactly what Go did or hardcode the logic 
    // if the files are mostly static. To be safe, we parse. But since I can't use serde_json easily without adding it... 
    // Actually, I can use the same `parse_float_fast` to parse the normalization.json.
    let mut cfg = Config {
        max_amount: 1.0 / 10000.0,
        max_installments: 1.0 / 12.0,
        amount_vs_avg_ratio: 1.0 / 10.0,
        max_minutes: 1.0 / 10000.0,
        max_km: 1.0 / 10000.0,
        max_tx_count_24h: 1.0 / 20.0,
        max_merchant_avg_amount: 1.0 / 10000.0,
        mcc_risk_arr: [0.5; 10000],
    };
    
    if let Ok(data) = std::fs::read_to_string("../../resources/normalization.json").or_else(|_| std::fs::read_to_string("resources/normalization.json")) {
        let d = data.as_bytes();
        let get = |k: &[u8]| -> f64 { let idx = find_direct(d, k); if idx >= 0 { 1.0 / parse_float_fast(d, idx as usize).0 } else { 1.0 } };
        cfg.max_amount = get(b"\"max_amount\"");
        cfg.max_installments = get(b"\"max_installments\"");
        cfg.amount_vs_avg_ratio = get(b"\"amount_vs_avg_ratio\"");
        cfg.max_minutes = get(b"\"max_minutes\"");
        cfg.max_km = get(b"\"max_km\"");
        cfg.max_tx_count_24h = get(b"\"max_tx_count_24h\"");
        cfg.max_merchant_avg_amount = get(b"\"max_merchant_avg_amount\"");
    }
    
    if let Ok(data) = std::fs::read_to_string("../../resources/mcc_risk.json").or_else(|_| std::fs::read_to_string("resources/mcc_risk.json")) {
        // Just extract all "1234": 0.5 pairs
        let mut i = 0;
        let d = data.as_bytes();
        while i < d.len() {
            if d[i] == b'"' {
                let (key, next) = parse_string_fast(d, i);
                i = next;
                if key.len() > 0 && key[0] >= b'0' && key[0] <= b'9' {
                    while i < d.len() && (d[i] == b':' || d[i] == b' ') { i += 1; }
                    let (val, next2) = parse_float_fast(d, i);
                    if let Ok(mcc) = std::str::from_utf8(key).unwrap_or("").parse::<usize>() {
                        if mcc < 10000 {
                            cfg.mcc_risk_arr[mcc] = val as f32;
                        }
                    }
                    i = next2;
                }
            } else {
                i += 1;
            }
        }
    }
    cfg
}

fn fast_vectorize(body: &[u8], cfg: &Config, q: &mut [f32; 14]) {
    let get_float = |idx: isize| -> f64 { if idx >= 0 { parse_float_fast(body, idx as usize).0 } else { 0.0 } };
    let get_int = |idx: isize| -> i64 { if idx >= 0 { parse_int_fast(body, idx as usize).0 } else { 0 } };
    let get_bool = |idx: isize| -> bool { if idx >= 0 { parse_bool_fast(body, idx as usize).0 } else { false } };
    let get_string = |idx: isize| -> &[u8] { if idx >= 0 { parse_string_fast(body, idx as usize).0 } else { &[] } };

    let amt = get_float(find_direct(body, b"\"amount\""));
    let inst = get_int(find_direct(body, b"\"installments\""));
    let req_at = get_string(find_direct(body, b"\"requested_at\""));

    let c_avg_amt = get_float(find_after(body, b"\"customer\"", b"\"avg_amount\""));
    let tx_count = get_int(find_direct(body, b"\"tx_count_24h\""));

    let known_start = find_direct(body, b"\"known_merchants\"");
    let mut known_block = &[] as &[u8];
    if known_start >= 0 && (known_start as usize) < body.len() && body[known_start as usize] == b'[' {
        let start_u = known_start as usize;
        if let Some(end) = body[start_u..].iter().position(|&x| x == b']') {
            known_block = &body[start_u..start_u + end + 1];
        }
    }

    let merch_id = get_string(find_after(body, b"\"merchant\"", b"\"id\""));
    let mcc_bytes = get_string(find_direct(body, b"\"mcc\""));
    let m_avg_amt = get_float(find_after(body, b"\"merchant\"", b"\"avg_amount\""));

    let is_online = get_bool(find_direct(body, b"\"is_online\""));
    let card_pres = get_bool(find_direct(body, b"\"card_present\""));
    let km_home = get_float(find_direct(body, b"\"km_from_home\""));

    let mut has_last = false;
    let mut last_ts = &[] as &[u8];
    let mut km_last = 0.0;

    if let Some(last_start) = body.windows(b"\"last_transaction\"".len()).position(|w| w == b"\"last_transaction\"") {
        let sub = &body[last_start..];
        let null_idx = sub.windows(4).position(|w| w == b"null").unwrap_or(usize::MAX);
        let time_idx = find_after(sub, b"\"last_transaction\"", b"\"timestamp\"");
        if time_idx >= 0 && (time_idx as usize) < null_idx {
            has_last = true;
            last_ts = parse_string_fast(sub, time_idx as usize).0;
            km_last = get_float(last_start as isize + find_after(sub, b"\"last_transaction\"", b"\"km_from_current\""));
        }
    }

    let mut known = false;
    if !known_block.is_empty() && !merch_id.is_empty() {
        known = known_block.windows(merch_id.len()).any(|w| w == merch_id);
    }

    let req_unix = fast_parse_time_str(req_at);
    let mut req_hour = (req_unix % 86400) / 3600;
    if req_hour < 0 { req_hour += 24; }
    let mut days = req_unix / 86400;
    if req_unix < 0 && req_unix % 86400 != 0 { days -= 1; }
    let mut req_wk = (days + 3) % 7;
    if req_wk < 0 { req_wk += 7; }

    q[0] = clamp(amt * cfg.max_amount);
    q[1] = clamp((inst as f64) * cfg.max_installments);
    if c_avg_amt > 0.0 {
        q[2] = clamp((amt / c_avg_amt) * cfg.amount_vs_avg_ratio);
    } else {
        q[2] = 1.0;
    }
    q[3] = req_hour as f32 / 23.0;
    q[4] = req_wk as f32 / 6.0;

    if !has_last || last_ts.is_empty() {
        q[5] = -1.0;
        q[6] = -1.0;
    } else {
        let last_unix = fast_parse_time_str(last_ts);
        let mins = (req_unix - last_unix) as f64 / 60.0;
        q[5] = clamp(mins * cfg.max_minutes);
        q[6] = clamp(km_last * cfg.max_km);
    }

    q[7] = clamp(km_home * cfg.max_km);
    q[8] = clamp((tx_count as f64) * cfg.max_tx_count_24h);
    q[9] = if is_online { 1.0 } else { 0.0 };
    q[10] = if card_pres { 1.0 } else { 0.0 };
    q[11] = if !known { 1.0 } else { 0.0 };

    let mut m = 0;
    for &b in mcc_bytes {
        if b >= b'0' && b <= b'9' {
            m = m * 10 + (b - b'0') as usize;
        }
    }
    if m < 10000 {
        q[12] = cfg.mcc_risk_arr[m];
    } else {
        q[12] = 0.5;
    }
    q[13] = clamp(m_avg_amt * cfg.max_merchant_avg_amount);
}

fn main() {
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN); }

    let cfg = load_config();
    let mut dataset = std::env::var("DATASET_PATH").unwrap_or_else(|_| "dataset.bin".to_string());
    if let Ok(shared) = std::env::var("SHARED_DATASET_PATH") {
        if let Ok(_) = std::fs::metadata(&shared) {
            dataset = shared;
        }
    }

    let c_path = CString::new(dataset).unwrap();
    let res = unsafe { init_engine(c_path.as_ptr()) };
    if res < 0 {
        panic!("failed init engine: {}", res);
    }

    let socket_path = std::env::var("SOCKET_PATH").unwrap_or_else(|_| "/tmp/sockets/api.sock".to_string());
    let _ = std::fs::remove_file(&socket_path);

    let uds_fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM, 0) };
    if uds_fd < 0 { panic!("socket err"); }
    
    let mut addr: libc::sockaddr_un = unsafe { mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let path_bytes = socket_path.as_bytes();
    unsafe {
        ptr::copy_nonoverlapping(path_bytes.as_ptr(), addr.sun_path.as_mut_ptr() as *mut u8, std::cmp::min(path_bytes.len(), 108));
    }
    if unsafe { libc::bind(uds_fd, &addr as *const _ as *const libc::sockaddr, mem::size_of::<libc::sockaddr_un>() as libc::socklen_t) } < 0 {
        panic!("bind err");
    }
    unsafe { libc::chmod(CString::new(socket_path).unwrap().as_ptr(), 0o777); }

    unsafe {
        let flags = libc::fcntl(uds_fd, libc::F_GETFL, 0);
        libc::fcntl(uds_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }

    let epfd = unsafe { libc::epoll_create1(0) };
    let mut ev = libc::epoll_event { events: libc::EPOLLIN as u32, u64: uds_fd as u64 };
    unsafe { libc::epoll_ctl(epfd, libc::EPOLL_CTL_ADD, uds_fd, &mut ev); }

    let mut events = vec![libc::epoll_event { events: 0, u64: 0 }; 4096];
    let mut buf = [0u8; 8192];
    let mut q_arr = [0.0f32; 14];

    // Per-connection buffer for Keep-Alive and TCP pipelining
    let max_fds = 10000;
    let mut conn_buffers: Vec<Vec<u8>> = vec![Vec::with_capacity(1024); max_fds];

    loop {
        let n = unsafe { libc::epoll_wait(epfd, events.as_mut_ptr(), 4096, -1) };
        if n < 0 { continue; }

        for i in 0..n {
            let fd = events[i as usize].u64 as RawFd;

            if fd == uds_fd {
                let mut cmsg_buf = [0u8; 1024];
                let mut iov_buf = [0u8; 1];
                let mut iov = libc::iovec { iov_base: iov_buf.as_mut_ptr() as *mut libc::c_void, iov_len: 1 };
                let mut msg: libc::msghdr = unsafe { mem::zeroed() };
                msg.msg_iov = &mut iov;
                msg.msg_iovlen = 1;
                msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
                msg.msg_controllen = cmsg_buf.len() as _;

                let rn = unsafe { libc::recvmsg(uds_fd, &mut msg, 0) };
                if rn <= 0 { continue; }

                unsafe {
                    let cmsg = libc::CMSG_FIRSTHDR(&msg);
                    if !cmsg.is_null() && (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                        let data = libc::CMSG_DATA(cmsg) as *mut libc::c_int;
                        let fd_count = ((*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / mem::size_of::<libc::c_int>();
                        for j in 0..fd_count {
                            let c_fd = *data.add(j);
                            if c_fd >= 0 && (c_fd as usize) < max_fds {
                                conn_buffers[c_fd as usize].clear();
                            }
                            let mut ev_c = libc::epoll_event { events: libc::EPOLLIN as u32, u64: c_fd as u64 };
                            libc::epoll_ctl(epfd, libc::EPOLL_CTL_ADD, c_fd, &mut ev_c);
                        }
                    }
                }
                continue;
            }

            let rn = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if rn <= 0 {
                unsafe {
                    if fd >= 0 && (fd as usize) < max_fds {
                        conn_buffers[fd as usize].clear();
                        conn_buffers[fd as usize].shrink_to_fit();
                    }
                    libc::epoll_ctl(epfd, libc::EPOLL_CTL_DEL, fd, ptr::null_mut());
                    libc::close(fd);
                }
                continue;
            }

            if fd < 0 || (fd as usize) >= max_fds { continue; }
            let cbuf = &mut conn_buffers[fd as usize];
            cbuf.extend_from_slice(&buf[..rn as usize]);

            let mut offset = 0;
            while offset < cbuf.len() {
                let chunk = &cbuf[offset..];
                if chunk.starts_with(b"GET /ready") {
                    unsafe { libc::write(fd, RESP_READY.as_ptr() as *const libc::c_void, RESP_READY.len()); }
                    offset += chunk.len();
                    break;
                } else if chunk.starts_with(b"POST /fraud-score") {
                    if let Some(body_idx) = chunk.windows(4).position(|w| w == b"\r\n\r\n") {
                        let mut cl = 0;
                        if let Some(cl_idx) = chunk.windows(16).position(|w| w == b"Content-Length: ") {
                            let (val, _) = parse_int_fast(chunk, cl_idx + 16);
                            cl = val as usize;
                        }
                        
                        let body_start = body_idx + 4;
                        if chunk.len() >= body_start + cl {
                            let body = &chunk[body_start..body_start + cl];
                            fast_vectorize(body, &cfg, &mut q_arr);
                            let frauds = unsafe { search_vector(q_arr.as_ptr(), 0) };
                            let resp = match frauds {
                                0 => RESP_0,
                                1 => RESP_1,
                                2 => RESP_2,
                                3 => RESP_3,
                                4 => RESP_4,
                                5 => RESP_5,
                                _ => RESP_3,
                            };
                            unsafe { libc::write(fd, resp.as_ptr() as *const libc::c_void, resp.len()); }
                            offset += body_start + cl;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                } else {
                    unsafe { libc::write(fd, RESP_404.as_ptr() as *const libc::c_void, RESP_404.len()); }
                    offset = cbuf.len(); // invalidate
                    break;
                }
            }
            
            if offset > 0 {
                if offset == cbuf.len() {
                    cbuf.clear();
                } else {
                    let remaining = cbuf.len() - offset;
                    cbuf.copy_within(offset.., 0);
                    cbuf.truncate(remaining);
                }
            }
        }
    }
}
