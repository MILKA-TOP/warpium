//! The adb server's smart-socket wire format: a request is its length as four hex digits followed
//! by the payload; the server answers `OKAY`, or `FAIL` followed by a length-prefixed message.

use std::io::{self, Read, Write};

use super::AdbError;

/// The longest request the adb server accepts.
const MAX_PAYLOAD: usize = 0xffff;

/// Writes one request.
pub fn send_request(writer: &mut impl Write, payload: &str) -> Result<(), AdbError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(AdbError::Protocol(format!(
            "request is too long ({} bytes)",
            payload.len()
        )));
    }
    let mut message = format!("{:04x}", payload.len()).into_bytes();
    message.extend_from_slice(payload.as_bytes());
    writer.write_all(&message)?;
    writer.flush()?;
    Ok(())
}

/// Reads the four-byte status. `FAIL` becomes [`AdbError::Fail`] with the server's message.
pub fn read_status(reader: &mut impl Read) -> Result<(), AdbError> {
    let mut status = [0u8; 4];
    reader.read_exact(&mut status).map_err(eof_as_closed)?;
    match &status {
        b"OKAY" => Ok(()),
        b"FAIL" => {
            let message = read_length_prefixed(reader).unwrap_or_default();
            Err(AdbError::Fail(message))
        }
        other => Err(AdbError::Protocol(format!(
            "unexpected status {:?}",
            String::from_utf8_lossy(other)
        ))),
    }
}

/// Reads a hex length followed by that many bytes, as (lossy) UTF-8.
pub fn read_length_prefixed(reader: &mut impl Read) -> Result<String, AdbError> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length).map_err(eof_as_closed)?;
    let length = std::str::from_utf8(&length)
        .ok()
        .and_then(|hex| usize::from_str_radix(hex, 16).ok())
        .ok_or_else(|| {
            AdbError::Protocol(format!(
                "invalid length {:?}",
                String::from_utf8_lossy(&length)
            ))
        })?;
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).map_err(eof_as_closed)?;
    Ok(String::from_utf8_lossy(&payload).into_owned())
}

fn eof_as_closed(err: io::Error) -> AdbError {
    if err.kind() == io::ErrorKind::UnexpectedEof {
        AdbError::Closed
    } else {
        AdbError::Io(err)
    }
}
