//! Reader for ILDA Image Data Transfer Format files (.ild), revision 011.
//! Formats 0, 1, 4 and 5 are frames, format 2 is a colour palette.
#![allow(dead_code)] // used by the .ild player (v0.3)

use crate::point::Point;

const HEADER_LEN: usize = 32;
const BLANKING: u8 = 0x40;

/// Appendix A of the spec: used by most files without a format 2 palette,
/// including the ILDA test pattern.
#[rustfmt::skip]
const DEFAULT_PALETTE: [[u8; 3]; 64] = [
    [255, 0, 0], [255, 16, 0], [255, 32, 0], [255, 48, 0],
    [255, 64, 0], [255, 80, 0], [255, 96, 0], [255, 112, 0],
    [255, 128, 0], [255, 144, 0], [255, 160, 0], [255, 176, 0],
    [255, 192, 0], [255, 208, 0], [255, 224, 0], [255, 240, 0],
    [255, 255, 0], [224, 255, 0], [192, 255, 0], [160, 255, 0],
    [128, 255, 0], [96, 255, 0], [64, 255, 0], [32, 255, 0],
    [0, 255, 0], [0, 255, 36], [0, 255, 73], [0, 255, 109],
    [0, 255, 146], [0, 255, 182], [0, 255, 219], [0, 255, 255],
    [0, 227, 255], [0, 198, 255], [0, 170, 255], [0, 142, 255],
    [0, 113, 255], [0, 85, 255], [0, 56, 255], [0, 28, 255],
    [0, 0, 255], [32, 0, 255], [64, 0, 255], [96, 0, 255],
    [128, 0, 255], [160, 0, 255], [192, 0, 255], [224, 0, 255],
    [255, 0, 255], [255, 32, 255], [255, 64, 255], [255, 96, 255],
    [255, 128, 255], [255, 160, 255], [255, 192, 255], [255, 224, 255],
    [255, 255, 255], [255, 224, 224], [255, 192, 192], [255, 160, 160],
    [255, 128, 128], [255, 96, 96], [255, 64, 64], [255, 32, 32],
];

/// Parses a whole .ild file into frames of points, in file order.
///
/// Z is dropped (front projection). A missing end-of-file header is accepted,
/// since many writers omit it. Colour indices beyond the palette draw white.
// ponytail: one palette for all projector numbers; per-projector palettes if
// multi-projector files ever matter.
pub fn parse(data: &[u8]) -> Result<Vec<Vec<Point>>, String> {
    let mut frames = Vec::new();
    let mut palette: Vec<[u8; 3]> = DEFAULT_PALETTE.to_vec();
    let mut at = 0;
    while at < data.len() {
        let header = data
            .get(at..at + HEADER_LEN)
            .ok_or_else(|| format!("truncated header at byte {at}"))?;
        if &header[0..4] != b"ILDA" {
            return Err(format!("no ILDA signature at byte {at}"));
        }
        let format = header[7];
        let records = u16::from_be_bytes([header[24], header[25]]) as usize;
        if records == 0 {
            break; // end of file
        }
        let len = match format {
            0 => 8,
            1 => 6,
            2 => 3,
            4 => 10,
            5 => 8,
            f => return Err(format!("unsupported format code {f} at byte {at}")),
        };
        at += HEADER_LEN;
        let body = data
            .get(at..at + records * len)
            .ok_or_else(|| format!("truncated section at byte {at}"))?;
        at += records * len;
        if format == 2 {
            palette = body.as_chunks::<3>().0.to_vec();
            continue;
        }
        let points = body.chunks_exact(len).map(|r| {
            let x = i16::from_be_bytes([r[0], r[1]]) as f32 / 32768.0;
            let y = i16::from_be_bytes([r[2], r[3]]) as f32 / 32768.0;
            // Status follows Z in the 3D formats.
            let s = if format == 0 || format == 4 { 6 } else { 4 };
            if r[s] & BLANKING != 0 {
                return Point::blank(x, y);
            }
            let [red, green, blue] = match format {
                0 | 1 => *palette.get(r[s + 1] as usize).unwrap_or(&[255; 3]),
                _ => [r[s + 3], r[s + 2], r[s + 1]], // stored blue, green, red
            };
            Point::lit(x, y, [red, green, blue].map(|c| c as f32 / 255.0))
        });
        frames.push(points.collect());
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(format: u8, records: &[&[u8]]) -> Vec<u8> {
        let mut v = b"ILDA\0\0\0".to_vec();
        v.push(format);
        v.extend_from_slice(&[0; 16]); // frame and company name
        v.extend_from_slice(&(records.len() as u16).to_be_bytes());
        v.extend_from_slice(&[0; 6]); // frame number, total frames, projector, reserved
        for r in records {
            v.extend_from_slice(r);
        }
        v
    }

    fn xy(x: i16, y: i16) -> Vec<u8> {
        [x.to_be_bytes(), y.to_be_bytes()].concat()
    }

    #[test]
    fn true_colour_frames() {
        let p0 = [xy(-32768, 32767), vec![0, 10, 20, 30]].concat(); // 2D, B G R
        let p1 = [xy(0, 0), vec![0x40 | 0x80, 255, 255, 255]].concat(); // blanked, last
        let p2 = [xy(16384, -16384), vec![0, 0, 0x80, 0, 0, 255]].concat(); // 3D
        let mut file = section(5, &[&p0, &p1]);
        file.extend(section(4, &[&p2]));
        file.extend(section(5, &[]));
        file.extend(b"trailing bytes after the end header are ignored");

        let frames = parse(&file).unwrap();
        assert_eq!(frames.len(), 2);
        let a = frames[0][0];
        assert_eq!((a.x, a.y, a.blank), (-1.0, 32767.0 / 32768.0, false));
        assert_eq!((a.r, a.g, a.b), (30.0 / 255.0, 20.0 / 255.0, 10.0 / 255.0));
        assert!(frames[0][1].blank && frames[0][1].r == 0.0);
        let c = frames[1][0];
        assert_eq!((c.x, c.y, c.r, c.g, c.b), (0.5, -0.5, 1.0, 0.0, 0.0));
    }

    #[test]
    fn indexed_colour_and_palettes() {
        let yellow = [xy(0, 0), vec![0, 16]].concat(); // format 1, default index 16
        let beyond = [xy(0, 0), vec![0, 200]].concat();
        let custom = [xy(0, 0), vec![0, 0, 0, 1]].concat(); // format 0 (3D), index 1
        let mut file = section(1, &[&yellow, &beyond]);
        file.extend(section(2, &[&[1, 2, 3], &[0, 128, 255]]));
        file.extend(section(0, &[&custom]));
        // No end-of-file header.

        let frames = parse(&file).unwrap();
        assert_eq!(frames.len(), 2);
        let y = frames[0][0];
        assert_eq!((y.r, y.g, y.b), (1.0, 1.0, 0.0));
        assert_eq!(frames[0][1].g, 1.0); // white beyond the palette
        let c = frames[1][0];
        assert_eq!((c.r, c.g, c.b), (0.0, 128.0 / 255.0, 1.0));
    }

    #[test]
    fn rejects_broken_files() {
        assert!(parse(b"ILDB").unwrap_err().contains("header"));
        let mut bad = section(5, &[]);
        bad[0] = b'X';
        assert!(parse(&bad).unwrap_err().contains("signature"));
        assert!(
            parse(&section(3, &[&[0; 8]]))
                .unwrap_err()
                .contains("format code 3")
        );
        let mut short = section(5, &[&[0; 8]]);
        short.pop();
        assert!(parse(&short).unwrap_err().contains("truncated section"));
        assert_eq!(parse(&[]).unwrap().len(), 0);
    }
}
