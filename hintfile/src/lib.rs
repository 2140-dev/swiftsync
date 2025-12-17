use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
};

type BlockHeight = u32;
type FilePos = u64;

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

#[derive(Debug)]
pub struct Hints {
    map: BTreeMap<BlockHeight, FilePos>,
    file: File,
    stop_height: BlockHeight,
}

impl Hints {
    // # Panics
    //
    // Panics when expected data is not present, or the hintfile overflows the maximum blockheight
    pub fn from_file(mut file: File) -> Self {
        let mut map = BTreeMap::new();
        let mut magic = [0; 4];
        file.read_exact(&mut magic).unwrap();
        assert_eq!(magic, [0x55, 0x54, 0x58, 0x4f]);
        let mut ver = [0; 1];
        file.read_exact(&mut ver).unwrap();
        if u8::from_le_bytes(ver) != 0x00 {
            panic!("Unsupported file version.");
        }
        let mut stop_height = [0; 4];
        file.read_exact(&mut stop_height).expect("empty file");
        let stop_height = BlockHeight::from_le_bytes(stop_height);
        for _ in 1..=stop_height {
            let mut height = [0; 4];
            file.read_exact(&mut height)
                .expect("expected kv pair does not exist.");
            let height = BlockHeight::from_le_bytes(height);
            let mut file_pos = [0; 8];
            file.read_exact(&mut file_pos)
                .expect("expected kv pair does not exist.");
            let file_pos = FilePos::from_le_bytes(file_pos);
            map.insert(height, file_pos);
        }
        Self {
            map,
            file,
            stop_height,
        }
    }

    /// Get the stop height of the hint file.
    pub fn stop_height(&self) -> BlockHeight {
        self.stop_height
    }

    /// # Panics
    ///
    /// If there are no offset present at that height, aka an overflow, or the entry has already
    /// been fetched.
    pub fn get_indexes(&mut self, height: BlockHeight) -> Vec<u64> {
        let file_pos = self
            .map
            .get(&height)
            .cloned()
            .expect("block height overflow");
        self.file
            .seek(SeekFrom::Start(file_pos))
            .expect("missing file position.");
        let mut bits_arr = [0; 4];
        self.file.read_exact(&mut bits_arr).unwrap();
        let mut unspents = Vec::new();
        let num_bits = u32::from_le_bytes(bits_arr);
        let mut curr_byte: u8 = 0;
        for bit_pos in 0..num_bits {
            let leftovers = bit_pos % 8;
            if leftovers == 0 {
                let mut single_byte_arr = [0; 1];
                self.file.read_exact(&mut single_byte_arr).unwrap();
                curr_byte = u8::from_le_bytes(single_byte_arr);
            }
            if ((curr_byte >> leftovers) & 0x01) == 0x01 {
                unspents.push(bit_pos as u64);
            }
        }
        unspents
    }
}

#[cfg(test)]
mod tests {
    use crate::{read_compact_size, write_compact_size};

    #[test]
    fn deser_roundtrip() {
        let mut buf = Vec::new();
        let less: u8 = 0xFB;
        write_compact_size(less as u64, &mut buf).unwrap();
        let read_cs = read_compact_size(&mut buf.as_slice()).unwrap();
        let cast_less = read_cs as u8;
        assert_eq!(less, cast_less);
        let mut buf = Vec::new();
        let median: u16 = 0xFFF;
        write_compact_size(median as u64, &mut buf).unwrap();
        let read_cs = read_compact_size(&mut buf.as_slice()).unwrap();
        let cast_median = read_cs as u16;
        assert_eq!(median, cast_median);
        let mut buf = Vec::new();
        let more: u32 = 0xFFFFF;
        write_compact_size(more as u64, &mut buf).unwrap();
        let read_cs = read_compact_size(&mut buf.as_slice()).unwrap();
        let cast_more = read_cs as u32;
        assert_eq!(more, cast_more);
    }
}
