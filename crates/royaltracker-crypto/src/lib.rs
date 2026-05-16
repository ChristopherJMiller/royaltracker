use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;
use zeroize::Zeroize;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid key length: expected 32 bytes, got {0}")]
    InvalidKeyLen(usize),
    #[error("invalid nonce length: expected 12 bytes, got {0}")]
    InvalidNonceLen(usize),
    #[error("base64 decode: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("aead: encryption/decryption failed")]
    Aead,
}

/// Wraps a single AEAD key used to encrypt user RCG passwords at rest.
/// Loaded once at startup from config; never serialized or logged.
pub struct Cipher {
    inner: ChaCha20Poly1305,
}

impl Cipher {
    pub fn from_base64(s: &str) -> Result<Self, CryptoError> {
        let mut bytes = base64::engine::general_purpose::STANDARD.decode(s.trim())?;
        if bytes.len() != 32 {
            let n = bytes.len();
            bytes.zeroize();
            return Err(CryptoError::InvalidKeyLen(n));
        }
        let key = Key::from_slice(&bytes);
        let inner = ChaCha20Poly1305::new(key);
        bytes.zeroize();
        Ok(Self { inner })
    }

    /// Generate a fresh random 32-byte key, returned base64-encoded for storage in config.
    pub fn generate_key_b64() -> String {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        let s = base64::engine::general_purpose::STANDARD.encode(key);
        key.zeroize();
        s
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = self
            .inner
            .encrypt(nonce, plaintext)
            .map_err(|_| CryptoError::Aead)?;
        Ok((nonce_bytes.to_vec(), ct))
    }

    pub fn decrypt(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if nonce.len() != 12 {
            return Err(CryptoError::InvalidNonceLen(nonce.len()));
        }
        let nonce = Nonce::from_slice(nonce);
        self.inner
            .decrypt(nonce, ciphertext)
            .map_err(|_| CryptoError::Aead)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = Cipher::generate_key_b64();
        let c = Cipher::from_base64(&key).unwrap();
        let pt = b"hunter2";
        let (nonce, ct) = c.encrypt(pt).unwrap();
        let out = c.decrypt(&nonce, &ct).unwrap();
        assert_eq!(out, pt);
    }

    #[test]
    fn tamper_detected() {
        let c = Cipher::from_base64(&Cipher::generate_key_b64()).unwrap();
        let (nonce, mut ct) = c.encrypt(b"secret").unwrap();
        ct[0] ^= 0xff;
        assert!(c.decrypt(&nonce, &ct).is_err());
    }

    #[test]
    fn rejects_wrong_key_size() {
        let short = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        assert!(matches!(
            Cipher::from_base64(&short),
            Err(CryptoError::InvalidKeyLen(16))
        ));
    }
}
