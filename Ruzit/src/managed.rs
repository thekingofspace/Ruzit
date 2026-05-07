
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;

const FILE_MAGIC: &[u8; 4] = b"RZMG";
const FILE_VERSION: u8 = 1;
const NONCE_LEN: usize = 12;

const KEY_PART_A: [u8; 32] = [
    0x4a, 0x91, 0x3c, 0x6e, 0xd5, 0x2f, 0x88, 0xb1, 0x07, 0x59, 0xe2, 0xa4, 0x6f, 0x18, 0xd3, 0x7c,
    0x91, 0x4d, 0xa6, 0x0e, 0x52, 0x88, 0xc1, 0xf3, 0x2a, 0x6e, 0xb5, 0x09, 0xd4, 0x77, 0x21, 0x9e,
];
const KEY_PART_B: [u8; 32] = [
    0x83, 0x12, 0x6e, 0xa9, 0x71, 0x4f, 0xc2, 0x06, 0xb5, 0x73, 0x1d, 0x88, 0x2c, 0xe7, 0x40, 0x91,
    0xf3, 0x05, 0x82, 0xea, 0x14, 0x67, 0xa9, 0x3b, 0xe8, 0x21, 0x6f, 0xc4, 0x52, 0x9c, 0xb1, 0x67,
];

fn derive_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    for i in 0..32 {
        k[i] = KEY_PART_A[i] ^ KEY_PART_B[i];
    }
    k
}

fn cipher() -> Aes256Gcm {
    let k = derive_key();
    let key = Key::<Aes256Gcm>::from_slice(&k);
    Aes256Gcm::new(key)
}

pub fn encrypt_payload(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);

    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher()
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("encrypt: {e}"))?;

    let mut out = Vec::with_capacity(4 + 1 + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(FILE_MAGIC);
    out.push(FILE_VERSION);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn decrypt_payload(blob: &[u8]) -> Result<Vec<u8>, String> {
    if blob.len() < 4 + 1 + NONCE_LEN + 16 {
        return Err("managed payload too small".into());
    }
    if &blob[..4] != FILE_MAGIC {
        return Err("not a Ruzit managed payload (bad magic)".into());
    }
    let version = blob[4];
    if version != FILE_VERSION {
        return Err(format!("unsupported managed payload version {version}"));
    }
    let nonce_bytes = &blob[5..5 + NONCE_LEN];
    let ciphertext = &blob[5 + NONCE_LEN..];

    let nonce = Nonce::from_slice(nonce_bytes);
    cipher()
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("decrypt: {e} (file tampered, key mismatch, or wrong format)"))
}
