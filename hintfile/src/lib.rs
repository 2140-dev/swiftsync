use std::io::{self, Read, Write};

pub fn write_compact_size<W: Write>(value: u64, writer: &mut W) -> Result<(), io::Error> {
    match value {
        0..=0xFC => {
            writer.write_all(&[value as u8]) // Cast ok because of match.
        }
        0xFD..=0xFFFF => {
            let v = (value as u16).to_le_bytes(); // Cast ok because of match.
            writer.write_all(&[0xFD, v[0], v[1]])
        }
        0x10000..=0xFFFFFFFF => {
            let v = (value as u32).to_le_bytes(); // Cast ok because of match.
            writer.write_all(&[0xFE, v[0], v[1], v[2], v[3]])
        }
        _ => panic!("unexpected large offset"),
    }
}

pub fn read_compact_size<R: Read>(reader: &mut R) -> Result<u64, io::Error> {
    let mut buf: [u8; 1] = [0; 1];
    reader.read_exact(&mut buf)?;
    let prefix = buf[0];
    match prefix {
        0xFD => {
            let mut buf: [u8; 2] = [0; 2];
            reader.read_exact(&mut buf)?;
            Ok(u16::from_le_bytes(buf) as u64)
        }
        0xFE => {
            let mut buf: [u8; 4] = [0; 4];
            reader.read_exact(&mut buf)?;
            Ok(u32::from_le_bytes(buf) as u64)
        }
        0..=0xFC => Ok(prefix as u64),
        _ => panic!("unexpected large offset"),
    }
}
