use anyhow::{anyhow, bail, Context, Result};
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};

pub const NONCE_LEN: usize = 12;
pub const KEY_LEN: usize = 32;
pub const TAG_LEN: u64 = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionKey {
    bytes: [u8; KEY_LEN],
}

impl EncryptionKey {
    pub fn from_hex(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if raw.len() != KEY_LEN * 2 {
            bail!("TELEGRAM_ENCRYPTION_KEY must be 64 hex characters");
        }
        let mut bytes = [0_u8; KEY_LEN];
        for (i, chunk) in raw.as_bytes().chunks_exact(2).enumerate() {
            let hi = hex_value(chunk[0]).ok_or_else(|| anyhow!("invalid hex digit"))?;
            let lo = hex_value(chunk[1]).ok_or_else(|| anyhow!("invalid hex digit"))?;
            bytes[i] = (hi << 4) | lo;
        }
        Ok(Self { bytes })
    }

    pub fn encrypt(&self, plaintext: &[u8], aad: &str) -> Result<EncryptedPayload> {
        let mut nonce = [0_u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| anyhow!("failed to generate encryption nonce"))?;
        let key = LessSafeKey::new(
            UnboundKey::new(&aead::CHACHA20_POLY1305, &self.bytes)
                .map_err(|_| anyhow!("invalid encryption key"))?,
        );
        let mut ciphertext = plaintext.to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad.as_bytes()),
            &mut ciphertext,
        )
        .map_err(|_| anyhow!("encryption failed"))?;
        Ok(EncryptedPayload {
            nonce_hex: hex_encode(&nonce),
            ciphertext,
        })
    }

    pub fn decrypt(&self, nonce_hex: &str, aad: &str, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let nonce = parse_nonce(nonce_hex)?;
        let key = LessSafeKey::new(
            UnboundKey::new(&aead::CHACHA20_POLY1305, &self.bytes)
                .map_err(|_| anyhow!("invalid encryption key"))?,
        );
        let mut bytes = ciphertext.to_vec();
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad.as_bytes()),
                &mut bytes,
            )
            .map_err(|_| anyhow!("decryption failed"))?;
        Ok(plaintext.to_vec())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedPayload {
    pub nonce_hex: String,
    pub ciphertext: Vec<u8>,
}

pub fn max_plaintext_size(max_upload_size: u64, encrypted: bool) -> Result<u64> {
    if !encrypted {
        return Ok(max_upload_size);
    }
    max_upload_size
        .checked_sub(TAG_LEN)
        .filter(|v| *v > 0)
        .ok_or_else(|| anyhow!("telegram_max_file_size must be greater than AEAD tag size"))
}

pub fn decrypt_optional(
    key: Option<&EncryptionKey>,
    nonce: Option<&str>,
    aad: &str,
    bytes: Vec<u8>,
) -> Result<Vec<u8>> {
    let Some(nonce) = nonce.filter(|s| !s.trim().is_empty()) else {
        return Ok(bytes);
    };
    let key =
        key.ok_or_else(|| anyhow!("encrypted Telegram payload requires TELEGRAM_ENCRYPTION_KEY"))?;
    key.decrypt(nonce, aad, &bytes)
        .with_context(|| format!("decrypting Telegram payload {aad}"))
}

pub fn db_sync_aad(snapshot_id: &str, part_index: i64) -> String {
    format!("db-sync/{snapshot_id}/{part_index}")
}

fn parse_nonce(raw: &str) -> Result<[u8; NONCE_LEN]> {
    let raw = raw.trim();
    if raw.len() != NONCE_LEN * 2 {
        bail!("encryption nonce must be 24 hex characters");
    }
    let mut bytes = [0_u8; NONCE_LEN];
    for (i, chunk) in raw.as_bytes().chunks_exact(2).enumerate() {
        let hi = hex_value(chunk[0]).ok_or_else(|| anyhow!("invalid nonce hex digit"))?;
        let lo = hex_value(chunk[1]).ok_or_else(|| anyhow!("invalid nonce hex digit"))?;
        bytes[i] = (hi << 4) | lo;
    }
    Ok(bytes)
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> EncryptionKey {
        EncryptionKey::from_hex(&"11".repeat(KEY_LEN)).unwrap()
    }

    #[test]
    fn round_trip_decrypts_with_matching_inputs() {
        let key = key();
        let encrypted = key.encrypt(b"segment bytes", "video_0/seg.m4s").unwrap();
        assert_ne!(encrypted.ciphertext, b"segment bytes");
        assert_eq!(encrypted.nonce_hex.len(), NONCE_LEN * 2);

        let plain = key
            .decrypt(
                &encrypted.nonce_hex,
                "video_0/seg.m4s",
                &encrypted.ciphertext,
            )
            .unwrap();
        assert_eq!(plain, b"segment bytes");
    }

    #[test]
    fn rejects_wrong_key_nonce_or_aad() {
        let key = key();
        let encrypted = key.encrypt(b"payload", "logical-key").unwrap();
        let wrong_key = EncryptionKey::from_hex(&"22".repeat(KEY_LEN)).unwrap();
        assert!(wrong_key
            .decrypt(&encrypted.nonce_hex, "logical-key", &encrypted.ciphertext)
            .is_err());
        assert!(key
            .decrypt(
                &"00".repeat(NONCE_LEN),
                "logical-key",
                &encrypted.ciphertext
            )
            .is_err());
        assert!(key
            .decrypt(&encrypted.nonce_hex, "other-key", &encrypted.ciphertext)
            .is_err());
    }

    #[test]
    fn validates_hex_key() {
        assert!(EncryptionKey::from_hex(&"aa".repeat(KEY_LEN)).is_ok());
        assert!(EncryptionKey::from_hex("abc").is_err());
        assert!(EncryptionKey::from_hex(&format!("{}zz", "aa".repeat(KEY_LEN - 1))).is_err());
    }
}
