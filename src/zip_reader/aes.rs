//! `WinZip` AE-1/AE-2 envelope around a bounded compressed stream.
//! Format: <https://www.winzip.com/en/support/aes-encryption/>
//! Cipher/MAC/KDF primitives come from `RustCrypto`. The stream composition and
//! EOF-authentication behavior follow zip-rs 9.0.5 (MIT; bundled notices).
use ::aes::cipher::{BlockCipherEncrypt, KeyInit};
use hmac::{HmacReset, Mac};
use sha1::Sha1;
use std::io::{self, Read};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Copy)]
pub(super) struct Settings {
    key_bytes: usize,
    pub(super) check_crc: bool,
}
pub(super) fn settings<R: Read>(entry: &zip::read::ZipFile<'_, R>) -> io::Result<Option<Settings>> {
    let mut settings = None;
    for field in entry.extra_data_fields() {
        if let zip::extra_fields::ExtraField::AeXEncryption(aes) = field {
            if settings.is_some() {
                return Err(io::Error::other("Duplicate ZIP AES settings"));
            }
            let mut serialized = [0; 11];
            aes.write(&mut serialized.as_mut_slice())
                .map_err(io::Error::other)?;
            let data = &serialized[4..];
            let version = u16::from_le_bytes([data[0], data[1]]);
            let key_bytes = match data[4] {
                1 => 16,
                2 => 24,
                3 => 32,
                _ => return Err(io::Error::other("Unsupported ZIP AES key strength")),
            };
            if !matches!(version, 1 | 2) {
                return Err(io::Error::other("Unsupported ZIP AES vendor version"));
            }
            settings = Some(Settings {
                key_bytes,
                check_crc: version == 1,
            });
        }
    }
    Ok(settings)
}

enum Cipher {
    Aes128(::aes::Aes128),
    Aes192(::aes::Aes192),
    Aes256(::aes::Aes256),
}
impl Cipher {
    fn new(key: &[u8]) -> io::Result<Self> {
        let error = |_| io::Error::other("Invalid ZIP AES key length");
        match key.len() {
            16 => ::aes::Aes128::new_from_slice(key)
                .map(Self::Aes128)
                .map_err(error),
            24 => ::aes::Aes192::new_from_slice(key)
                .map(Self::Aes192)
                .map_err(error),
            32 => ::aes::Aes256::new_from_slice(key)
                .map(Self::Aes256)
                .map_err(error),
            _ => Err(io::Error::other("Invalid ZIP AES key length")),
        }
    }
    fn block(&self, data: &mut [u8; 16]) {
        let mut block = (*data).into();
        match self {
            Self::Aes128(cipher) => cipher.encrypt_block(&mut block),
            Self::Aes192(cipher) => cipher.encrypt_block(&mut block),
            Self::Aes256(cipher) => cipher.encrypt_block(&mut block),
        }
        data.copy_from_slice(&block);
    }
}

pub(super) struct State {
    cipher: Cipher,
    hmac: HmacReset<Sha1>,
    remaining: u64,
    // WinZip uses a 128-bit little-endian counter beginning at one, no nonce.
    counter: u128,
    block: [u8; 16],
    position: usize,
    finalized: bool,
    authenticated: bool,
}
impl State {
    pub(super) fn new<R: Read>(
        reader: &mut R,
        password: &[u8],
        total: u64,
        settings: Settings,
    ) -> io::Result<Self> {
        let key_len = settings.key_bytes;
        let salt_len = key_len / 2;
        let remaining = total
            .checked_sub((salt_len + 12) as u64)
            .ok_or_else(|| io::Error::other("Invalid ZIP AES compressed size"))?;
        let mut salt = [0; 16];
        reader.read_exact(&mut salt[..salt_len])?;
        let mut verifier = [0; 2];
        reader.read_exact(&mut verifier)?;
        let mut derived = Zeroizing::new([0; 66]);
        let derived_len = 2 * key_len + 2;
        pbkdf2::pbkdf2::<HmacReset<Sha1>>(
            password,
            &salt[..salt_len],
            1000,
            &mut derived[..derived_len],
        )
        .map_err(io::Error::other)?;
        if !constant_time_eq::constant_time_eq(&verifier, &derived[derived_len - 2..derived_len]) {
            return Err(io::Error::other("Invalid ZIP password"));
        }
        let cipher = Cipher::new(&derived[..key_len])?;
        let hmac = HmacReset::<Sha1>::new_from_slice(&derived[key_len..key_len * 2])
            .map_err(io::Error::other)?;
        Ok(Self {
            cipher,
            hmac,
            remaining,
            counter: 1,
            block: [0; 16],
            position: 16,
            finalized: false,
            authenticated: false,
        })
    }
    fn crypt(&mut self, bytes: &mut [u8]) {
        for byte in bytes {
            if self.position == 16 {
                self.block = self.counter.to_le_bytes();
                self.cipher.block(&mut self.block);
                // A ZIP entry has at most u64::MAX bytes, so it cannot wrap a u128 block counter.
                self.counter += 1;
                self.position = 0;
            }
            *byte ^= self.block[self.position];
            self.position += 1;
        }
    }
    fn authenticate<R: Read>(&mut self, reader: &mut R) -> io::Result<()> {
        if self.finalized {
            return if self.authenticated {
                Ok(())
            } else {
                Err(auth_error())
            };
        }
        // Poison before reading so a truncated/bad tag can never become a clean EOF on retry.
        self.finalized = true;
        let mut expected = [0; 10];
        reader.read_exact(&mut expected)?;
        let actual = self.hmac.finalize_reset().into_bytes();
        if !constant_time_eq::constant_time_eq(&expected, &actual[..10]) {
            return Err(auth_error());
        }
        self.authenticated = true;
        Ok(())
    }
    pub(super) fn read<R: Read>(&mut self, reader: &mut R, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.finalized && !self.authenticated {
            return Err(auth_error());
        }
        if self.remaining == 0 {
            self.authenticate(reader)?;
            return Ok(0);
        }
        let length = self.remaining.min(bytes.len() as u64) as usize;
        let count = reader.read(&mut bytes[..length])?;
        if count == 0 {
            self.finalized = true;
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Truncated ZIP AES ciphertext",
            ));
        }
        self.remaining -= count as u64;
        self.hmac.update(&bytes[..count]);
        self.crypt(&mut bytes[..count]);
        if self.remaining == 0 {
            self.authenticate(reader)?;
        }
        Ok(count)
    }
    pub(super) fn finish<R: Read>(&mut self, reader: &mut R) -> io::Result<()> {
        // A codec may stop at EOS with authenticated ciphertext still unread.
        // Buffer contents already read by the codec have already been MACed.
        let mut discard = [0; 8192];
        while self.read(reader, &mut discard)? != 0 {}
        Ok(())
    }
}
fn auth_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "ZIP AES authentication failed (wrong password or damaged data)",
    )
}
impl Drop for State {
    fn drop(&mut self) {
        self.block.zeroize();
    }
}
