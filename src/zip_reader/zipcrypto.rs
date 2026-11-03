//! Traditional ZIP decryption, independently implemented from PKWARE APPNOTE
//! 6.1 for compatibility. This is not a cipher for creating new secure archives.

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut value = i as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ if value & 1 != 0 { 0xedb8_8320 } else { 0 };
            bit += 1;
        }
        table[i] = value;
        i += 1;
    }
    table
}
const CRC: [u32; 256] = crc_table();
fn crc(key: u32, byte: u8) -> u32 {
    (key >> 8) ^ CRC[((key as u8) ^ byte) as usize]
}

pub(super) struct Keys([u32; 3]);
impl Keys {
    pub(super) fn new(password: &[u8]) -> Self {
        let mut keys = Self([0x1234_5678, 0x2345_6789, 0x3456_7890]);
        for &byte in password {
            keys.update(byte);
        }
        keys
    }
    fn update(&mut self, plain: u8) {
        self.0[0] = crc(self.0[0], plain);
        self.0[1] = self.0[1]
            .wrapping_add(self.0[0] & 255)
            .wrapping_mul(134_775_813)
            .wrapping_add(1);
        self.0[2] = crc(self.0[2], (self.0[1] >> 24) as u8);
    }
    pub(super) fn decrypt(&mut self, bytes: &mut [u8]) {
        for byte in bytes {
            let value = (self.0[2] & 0xffff) | 2;
            *byte ^= (value.wrapping_mul(value ^ 1) >> 8) as u8;
            self.update(*byte);
        }
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.0.zeroize();
    }
}
