//! Enttec DMX USB Pro protocol over a serial port (receive side).
//! Open DMX clones cannot receive and are not supported.

use super::Publisher;
use std::io::Read;
use std::time::Duration;

const SOM: u8 = 0x7e;
const EOM: u8 = 0xe7;
const LABEL_RECEIVED_DMX: u8 = 5;
const LABEL_RECEIVE_ON_CHANGE: u8 = 8;

pub fn message(label: u8, data: &[u8]) -> Vec<u8> {
    let mut m = vec![SOM, label];
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(data);
    m.push(EOM);
    m
}

/// Pulls complete messages out of `buf`, consuming what was parsed.
/// Returns DMX data (without start code) for each "Received DMX" message.
pub fn drain(buf: &mut Vec<u8>, mut on_dmx: impl FnMut(&[u8])) {
    let mut pos = 0;
    while let Some(start) = buf[pos..].iter().position(|&b| b == SOM).map(|i| i + pos) {
        if buf.len() < start + 4 {
            pos = start;
            break;
        }
        let label = buf[start + 1];
        let len = u16::from_le_bytes([buf[start + 2], buf[start + 3]]) as usize;
        let end = start + 4 + len;
        if buf.len() <= end {
            pos = start;
            break;
        }
        if buf[end] != EOM {
            pos = start + 1; // not a real frame start, resync
            continue;
        }
        let data = &buf[start + 4..end];
        // data[0] = receive status, data[1] = start code
        if label == LABEL_RECEIVED_DMX && data.len() >= 2 && data[0] == 0 && data[1] == 0 {
            on_dmx(&data[2..]);
        }
        pos = end + 1;
    }
    buf.drain(..pos);
}

pub fn run(path: &str, mut out: Publisher) -> std::io::Result<()> {
    let mut port = serialport::new(path, 57600)
        .timeout(Duration::from_millis(100))
        .open()?;
    port.write_all(&message(LABEL_RECEIVE_ON_CHANGE, &[0]))?; // 0 = send every frame
    log::info!("DMX USB Pro on {path}");
    let mut acc = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        match port.read(&mut chunk) {
            Ok(0) => {}
            Ok(n) => {
                acc.extend_from_slice(&chunk[..n]);
                drain(&mut acc, |dmx| out.publish(dmx));
                if acc.len() > 4096 {
                    acc.clear(); // garbage without framing; start over
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_across_reads() {
        let m = message(LABEL_RECEIVED_DMX, &[0, 0, 9, 8, 7]);
        let mut buf = vec![0x11, 0x22]; // leading junk
        buf.extend_from_slice(&m[..5]);
        let mut got = Vec::new();
        drain(&mut buf, |d| got.push(d.to_vec()));
        assert!(got.is_empty());
        buf.extend_from_slice(&m[5..]);
        buf.extend_from_slice(&message(LABEL_RECEIVED_DMX, &[0, 0xcc, 1])); // wrong start code
        buf.extend_from_slice(&message(LABEL_RECEIVED_DMX, &[0, 0, 1]));
        drain(&mut buf, |d| got.push(d.to_vec()));
        assert_eq!(got, vec![vec![9, 8, 7], vec![1]]);
        assert!(buf.is_empty());
    }
}
