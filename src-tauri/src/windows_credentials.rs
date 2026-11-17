//! Windows current-user DPAPI boundary. No plaintext or cross-platform fallback.
//! Purpose/frame validation happens before a decoded secret reaches any client.
use sha2::{Digest, Sha256};
const ERROR: &str = "windows_credentials_unavailable";
pub(crate) type Result<T> = std::result::Result<T, &'static str>;
pub(crate) const MAX_PAYLOAD: usize = 16 * 1024;
const MAGIC: &[u8] = b"Sonante-Windows-Secret-v1\0";
const HEADER: usize = MAGIC.len() + 1 + 4;
const DIGEST: usize = 32;
const MAX_FRAME: usize = HEADER + MAX_PAYLOAD + DIGEST;
pub(crate) const MAX_SEALED: usize = MAX_FRAME + 4096;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Purpose {
    Navidrome,
    Plex,
    Scrobbling,
}
impl Purpose {
    fn tag(self) -> u8 {
        match self {
            Self::Navidrome => 1,
            Self::Plex => 2,
            Self::Scrobbling => 3,
        }
    }
    #[cfg(windows)]
    fn entropy(self) -> &'static [u8] {
        match self {
            Self::Navidrome => b"Sonante.Windows.Navidrome.v1",
            Self::Plex => b"Sonante.Windows.Plex.v1",
            Self::Scrobbling => b"Sonante.Windows.Scrobbling.v1",
        }
    }
}
pub(crate) struct Sensitive(Vec<u8>);
impl Sensitive {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
    pub(crate) fn writer(&mut self) -> impl std::io::Write + '_ {
        LimitedWriter(&mut self.0)
    }
}
struct LimitedWriter<'a>(&'a mut Vec<u8>);
impl std::io::Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .0
            .len()
            .checked_add(bytes.len())
            .is_none_or(|len| len > MAX_PAYLOAD)
        {
            return Err(std::io::Error::other("credential_size"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe { std::ptr::write_volatile(byte, 0) }
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}
impl Drop for Sensitive {
    fn drop(&mut self) {
        wipe(&mut self.0)
    }
}
trait Protector {
    fn seal(&self, purpose: Purpose, frame: &[u8]) -> Result<Vec<u8>>;
    fn open(&self, purpose: Purpose, sealed: &[u8]) -> Result<Sensitive>;
}
struct Native;
impl Protector for Native {
    fn seal(&self, purpose: Purpose, frame: &[u8]) -> Result<Vec<u8>> {
        if frame.is_empty() || frame.len() > MAX_FRAME {
            return Err(ERROR);
        }
        #[cfg(windows)]
        {
            native::apply(purpose, frame, false).map(|value| value.0.clone())
        }
        #[cfg(not(windows))]
        {
            let _ = purpose;
            Err(ERROR)
        }
    }
    fn open(&self, purpose: Purpose, sealed: &[u8]) -> Result<Sensitive> {
        if sealed.is_empty() || sealed.len() > MAX_SEALED {
            return Err(ERROR);
        }
        #[cfg(windows)]
        {
            native::apply(purpose, sealed, true)
        }
        #[cfg(not(windows))]
        {
            let _ = purpose;
            Err(ERROR)
        }
    }
}
fn seal_with(codec: &impl Protector, purpose: Purpose, payload: &[u8]) -> Result<Vec<u8>> {
    if payload.is_empty() || payload.len() > MAX_PAYLOAD {
        return Err(ERROR);
    }
    let mut frame = Sensitive::new(Vec::with_capacity(HEADER + payload.len() + DIGEST));
    frame.0.extend_from_slice(MAGIC);
    frame.0.push(purpose.tag());
    frame
        .0
        .extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.0.extend_from_slice(payload);
    let sum = Sha256::digest(frame.bytes());
    frame.0.extend_from_slice(&sum);
    let sealed = codec.seal(purpose, frame.bytes())?;
    if sealed.is_empty() || sealed.len() > MAX_SEALED {
        return Err(ERROR);
    }
    Ok(sealed)
}
fn open_with(codec: &impl Protector, purpose: Purpose, sealed: &[u8]) -> Result<Sensitive> {
    if sealed.is_empty() || sealed.len() > MAX_SEALED {
        return Err(ERROR);
    }
    let frame = codec.open(purpose, sealed)?;
    let bytes = frame.bytes();
    if bytes.len() < HEADER + DIGEST
        || bytes.len() > MAX_FRAME
        || &bytes[..MAGIC.len()] != MAGIC
        || bytes[MAGIC.len()] != purpose.tag()
    {
        return Err(ERROR);
    }
    let len = u32::from_le_bytes(
        bytes[MAGIC.len() + 1..HEADER]
            .try_into()
            .map_err(|_| ERROR)?,
    ) as usize;
    if len == 0 || len > MAX_PAYLOAD || bytes.len() != HEADER + len + DIGEST {
        return Err(ERROR);
    }
    let expected = Sha256::digest(&bytes[..HEADER + len]);
    let difference = expected
        .iter()
        .zip(&bytes[HEADER + len..])
        .fold(0u8, |all, (a, b)| all | (a ^ b));
    if difference != 0 {
        return Err(ERROR);
    }
    Ok(Sensitive::new(bytes[HEADER..HEADER + len].to_vec()))
}
pub(crate) fn seal(purpose: Purpose, payload: &[u8]) -> Result<Vec<u8>> {
    seal_with(&Native, purpose, payload)
}
pub(crate) fn open(purpose: Purpose, sealed: &[u8]) -> Result<Sensitive> {
    open_with(&Native, purpose, sealed)
}
#[cfg(windows)]
mod native {
    use super::*;
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    struct Output {
        blob: CRYPT_INTEGER_BLOB,
        secret: bool,
    }
    impl Drop for Output {
        fn drop(&mut self) {
            if !self.blob.pbData.is_null() {
                // A successful Windows call owns this allocation until LocalFree. Do not
                // inspect or adopt out parameters after a failed CryptProtect/Unprotect.
                if self.secret {
                    unsafe {
                        wipe(std::slice::from_raw_parts_mut(
                            self.blob.pbData,
                            self.blob.cbData as usize,
                        ))
                    }
                }
                if !unsafe { LocalFree(self.blob.pbData.cast()) }.is_null() {
                    eprintln!("Windows credentials: memory_release_failed");
                }
            }
        }
    }
    pub(super) fn apply(purpose: Purpose, bytes: &[u8], decrypt: bool) -> Result<Sensitive> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr().cast_mut(),
        };
        let entropy = purpose.entropy();
        let entropy = CRYPT_INTEGER_BLOB {
            cbData: entropy.len() as u32,
            pbData: entropy.as_ptr().cast_mut(),
        };
        let mut result = CRYPT_INTEGER_BLOB::default();
        let success = unsafe {
            if decrypt {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    &entropy,
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut result,
                )
            } else {
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    &entropy,
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut result,
                )
            }
        };
        if success == 0 {
            return Err(ERROR);
        }
        let result = Output {
            blob: result,
            secret: decrypt,
        };
        let limit = if decrypt { MAX_FRAME } else { MAX_SEALED };
        if result.blob.pbData.is_null()
            || result.blob.cbData == 0
            || result.blob.cbData as usize > limit
        {
            return Err(ERROR);
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(result.blob.pbData, result.blob.cbData as usize) };
        Ok(Sensitive::new(bytes.to_vec()))
    }
}

#[cfg(test)]
struct FixtureCodec;
#[cfg(test)]
impl Protector for FixtureCodec {
    fn seal(&self, _: Purpose, bytes: &[u8]) -> Result<Vec<u8>> {
        let mut result = b"NOT-CRYPTO-FIXTURE\0".to_vec();
        result.extend_from_slice(bytes);
        Ok(result)
    }
    fn open(&self, _: Purpose, bytes: &[u8]) -> Result<Sensitive> {
        Ok(Sensitive::new(
            bytes
                .strip_prefix(b"NOT-CRYPTO-FIXTURE\0")
                .ok_or(ERROR)?
                .to_vec(),
        ))
    }
}
#[cfg(test)]
pub(crate) fn fixture_seal(purpose: Purpose, bytes: &[u8]) -> Result<Vec<u8>> {
    seal_with(&FixtureCodec, purpose, bytes)
}
#[cfg(test)]
pub(crate) fn fixture_open(purpose: Purpose, bytes: &[u8]) -> Result<Sensitive> {
    open_with(&FixtureCodec, purpose, bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_roundtrip_is_bounded_and_binds_purpose_without_claiming_mock_encryption() {
        for purpose in [Purpose::Navidrome, Purpose::Plex, Purpose::Scrobbling] {
            let bytes = fixture_seal(purpose, b"fixture secret").unwrap();
            assert_eq!(
                fixture_open(purpose, &bytes).unwrap().bytes(),
                b"fixture secret"
            );
            for other in [Purpose::Navidrome,Purpose::Plex,Purpose::Scrobbling] {
                if other!=purpose {assert!(fixture_open(other,&bytes).is_err());}
            }
        }
        assert!(fixture_seal(Purpose::Navidrome, &[]).is_err());
        assert!(fixture_seal(Purpose::Navidrome, &vec![1; MAX_PAYLOAD + 1]).is_err());
        assert!(fixture_open(Purpose::Navidrome, &vec![0; MAX_SEALED + 1]).is_err());
    }
    #[test]
    fn every_corrupt_byte_and_truncation_fails_even_if_crypto_provider_returns_success() {
        let value = fixture_seal(Purpose::Navidrome, b"{\"password\":\"fixture\"}").unwrap();
        for index in 0..value.len() {
            let mut bad = value.clone();
            bad[index] ^= 1;
            assert!(
                fixture_open(Purpose::Navidrome, &bad).is_err(),
                "byte {index}"
            );
            assert!(fixture_open(Purpose::Navidrome, &value[..index]).is_err());
        }
        let mut extra = value;
        extra.push(0);
        assert!(fixture_open(Purpose::Navidrome, &extra).is_err());
    }
    #[test]
    fn serialization_writer_refuses_over_budget_before_reallocation() {
        use std::io::Write;
        let mut value = Sensitive::new(Vec::with_capacity(MAX_PAYLOAD + 1));
        {
            let mut writer = value.writer();
            writer.write_all(&vec![1; MAX_PAYLOAD]).unwrap();
            assert!(writer.write_all(&[2]).is_err());
        }
        assert_eq!(value.bytes().len(), MAX_PAYLOAD);
    }
    #[test]
    fn explicit_wipe_clears_all_initialized_bytes() {
        let mut bytes = vec![9u8; 256];
        wipe(&mut bytes);
        assert!(bytes.iter().all(|b| *b == 0));
    }
    #[cfg(not(windows))]
    #[test]
    fn native_boundary_on_non_windows_fails_without_plaintext_fallback() {
        assert!(seal(Purpose::Navidrome, b"fixture").is_err());
        assert!(open(Purpose::Navidrome, b"fixture").is_err());
    }
    #[cfg(windows)]
    #[test]
    fn native_current_user_dpapi_roundtrip_and_purpose_are_explicitly_native_only() {
        let sealed = seal(Purpose::Navidrome, b"fixed isolated credential fixture").unwrap();
        assert_ne!(sealed, b"fixed isolated credential fixture");
        assert_eq!(
            open(Purpose::Navidrome, &sealed).unwrap().bytes(),
            b"fixed isolated credential fixture"
        );
        assert!(open(Purpose::Plex, &sealed).is_err());
    }
}
