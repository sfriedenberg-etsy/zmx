//! Filesystem-safe session name encoding (percent-encoding) and socket path
//! construction.

const HEX_CHARS: &[u8; 16] = b"0123456789ABCDEF";

/// True for characters that are safe in filenames (don't need encoding).
fn is_filename_safe(ch: u8) -> bool {
    ch != b'/' && ch != b'\\' && ch != b'%' && ch != 0
}

/// Encode a session name to be filesystem-safe using percent-encoding.
pub fn encode_session_name(session_name: &str) -> String {
    let mut buf = String::with_capacity(session_name.len() * 3);
    for &ch in session_name.as_bytes() {
        if is_filename_safe(ch) {
            buf.push(ch as char);
        } else {
            buf.push('%');
            buf.push(HEX_CHARS[(ch >> 4) as usize] as char);
            buf.push(HEX_CHARS[(ch & 0x0F) as usize] as char);
        }
    }
    buf
}

fn hex_val(ch: u8) -> Option<u8> {
    match ch {
        b'0'..=b'9' => Some(ch - b'0'),
        b'a'..=b'f' => Some(ch - b'a' + 10),
        b'A'..=b'F' => Some(ch - b'A' + 10),
        _ => None,
    }
}

/// Decode a percent-encoded session name back to the original. Invalid
/// escapes pass through verbatim.
pub fn decode_session_name(encoded: &str) -> String {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Maximum sockaddr_un path length (excluding the NUL terminator).
fn max_socket_path() -> usize {
    let addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_path.len() - 1
}

pub fn socket_path(socket_dir: &str, session_name: &str) -> Result<String, String> {
    let encoded = encode_session_name(session_name);
    let fname = format!("{socket_dir}/{encoded}");
    let max_path = max_socket_path();
    if fname.len() > max_path {
        return Err(format!(
            "socket path too long ({} bytes, max {}): {}",
            fname.len(),
            max_path,
            fname
        ));
    }
    Ok(fname)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_session_name_encodes_slashes_and_percent_signs() {
        assert_eq!(encode_session_name("my-session"), "my-session");
        assert_eq!(encode_session_name("projects/web"), "projects%2Fweb");
        assert_eq!(encode_session_name("a/b/c"), "a%2Fb%2Fc");
        assert_eq!(encode_session_name("100%done"), "100%25done");
        assert_eq!(encode_session_name("win\\path"), "win%5Cpath");
    }

    #[test]
    fn decode_session_name_decodes_percent_encoded_characters() {
        assert_eq!(decode_session_name("my-session"), "my-session");
        assert_eq!(decode_session_name("projects%2Fweb"), "projects/web");
        assert_eq!(decode_session_name("a%2Fb%2Fc"), "a/b/c");
        assert_eq!(decode_session_name("100%25done"), "100%done");
    }

    #[test]
    fn encode_and_decode_are_inverse_operations() {
        let cases = [
            "simple",
            "with/slash",
            "multi/level/path",
            "percent%sign",
            "back\\slash",
            "mixed/path%with\\all",
        ];
        for original in cases {
            assert_eq!(
                decode_session_name(&encode_session_name(original)),
                original
            );
        }
    }
}
