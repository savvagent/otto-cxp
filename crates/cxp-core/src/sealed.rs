//! A sealed value as one byte string: `nonce || ciphertext`, from
//! [`otto_tenant::crypto::Cipher`] (AES-256-GCM with a fresh 96-bit nonce per
//! seal). The console's stored tokens and its OAuth flow cookie both use this
//! layout, so the one length check lives here.
//!
//! `Cipher` takes no associated data, so a sealed value is not bound to where
//! it is stored: a row's sealed token copied into another row still opens.
//! Only someone who can already write the table could do that, and the
//! session row it lands in is still checked against the platform on every
//! use (the token's org and user must be the row's).

use otto_tenant::crypto::Cipher;

use crate::error::Result;

/// The AES-GCM nonce length.
pub const NONCE_BYTES: usize = 12;

/// The AES-GCM authentication tag length: every ciphertext is at least this.
const TAG_BYTES: usize = 16;

/// Seal `plain` as `nonce || ciphertext`.
pub fn seal(cipher: &Cipher, plain: &[u8]) -> Result<Vec<u8>> {
    let sealed = cipher.seal(plain)?;
    let mut out = Vec::with_capacity(sealed.nonce.len() + sealed.ciphertext.len());
    out.extend_from_slice(&sealed.nonce);
    out.extend_from_slice(&sealed.ciphertext);
    Ok(out)
}

/// Open a `nonce || ciphertext` made by [`seal`]. Anything too short to be
/// one, sealed under another key, or tampered with is an error.
pub fn open(cipher: &Cipher, stored: &[u8]) -> Result<Vec<u8>> {
    if stored.len() < NONCE_BYTES + TAG_BYTES {
        return Err(otto_tenant::Error::Crypto("a sealed value is truncated".into()).into());
    }
    let (nonce, ciphertext) = stored.split_at(NONCE_BYTES);
    Ok(cipher.open(ciphertext, nonce)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher(byte: u8) -> Cipher {
        use base64::Engine;
        Cipher::from_base64_key(&base64::engine::general_purpose::STANDARD.encode([byte; 32]))
            .unwrap()
    }

    #[test]
    fn a_sealed_value_opens_only_whole_and_with_its_key() {
        let sealed = seal(&cipher(1), b"secret").unwrap();
        assert_eq!(open(&cipher(1), &sealed).unwrap(), b"secret");
        assert!(open(&cipher(2), &sealed).is_err(), "wrong key");
        for len in [0, 1, NONCE_BYTES, NONCE_BYTES + TAG_BYTES - 1] {
            assert!(open(&cipher(1), &sealed[..len]).is_err(), "{len} bytes");
        }
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open(&cipher(1), &tampered).is_err(), "tampered");
        // Even the empty plaintext is nonce + tag.
        assert_eq!(
            seal(&cipher(1), b"").unwrap().len(),
            NONCE_BYTES + TAG_BYTES
        );
    }
}
