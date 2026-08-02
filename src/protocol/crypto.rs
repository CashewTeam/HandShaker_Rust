use aes::Aes256;
use base64::Engine;
use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use md5::{Digest as _, Md5};
use rand::rngs::OsRng;
use rsa::pkcs1::EncodeRsaPublicKey;
use rsa::{Pkcs1v15Encrypt, Pkcs1v15Sign, RsaPrivateKey};
use sha2::Sha256;

use crate::error::{Error, Result};
use crate::i18n;

type Aes256CbcEncryptor = cbc::Encryptor<Aes256>;

pub(crate) const KEY_TABLE: [u8; 48] = [
    0x2b, 0x9e, 0x34, 0xd4, 0xe1, 0xd9, 0x08, 0x89, 0x94, 0x93, 0x9e, 0xc4, 0xe3, 0xe9, 0x60, 0xc5,
    0x28, 0xe3, 0xee, 0x32, 0xb0, 0xde, 0x27, 0xef, 0x6b, 0xc2, 0x97, 0x92, 0x05, 0x4e, 0xf9, 0x73,
    0x9c, 0xe8, 0xe8, 0x7b, 0xb4, 0x95, 0xf2, 0xea, 0x0d, 0x72, 0xd4, 0xf4, 0xf4, 0x0b, 0x3b, 0xde,
];

pub(crate) struct SessionKeys {
    private: RsaPrivateKey,
    public_der: Vec<u8>,
}

impl SessionKeys {
    pub fn generate() -> Result<Self> {
        let private = RsaPrivateKey::new(&mut OsRng, 1024).map_err(|error| {
            Error::Handshake(i18n::format(
                "crypto.rsa_generate_failed",
                &[&error.to_string()],
            ))
        })?;
        let public_der = private
            .to_public_key()
            .to_pkcs1_der()
            .map_err(|error| {
                Error::Handshake(i18n::format(
                    "crypto.rsa_public_encode_failed",
                    &[&error.to_string()],
                ))
            })?
            .as_bytes()
            .to_vec();
        Ok(Self {
            private,
            public_der,
        })
    }

    pub fn build_enckey(&self) -> Vec<u8> {
        build_enckey(&self.public_der)
    }

    pub fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
        let digest = Sha256::digest(data);
        self.private
            .sign(Pkcs1v15Sign::new::<Sha256>(), &digest)
            .map_err(|error| {
                Error::Protocol(i18n::format(
                    "crypto.rsa_sign_failed",
                    &[&error.to_string()],
                ))
            })
    }

    pub fn decrypt_handshake_result(&self, encoded: &[u8]) -> Result<Vec<u8>> {
        let normalized: Vec<u8> = encoded
            .iter()
            .copied()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect();
        let encrypted = base64::engine::general_purpose::STANDARD
            .decode(normalized)
            .map_err(|error| {
                Error::Handshake(i18n::format(
                    "crypto.handshake_base64_invalid",
                    &[&error.to_string()],
                ))
            })?;
        self.private
            .decrypt(Pkcs1v15Encrypt, &encrypted)
            .map_err(|error| {
                Error::Handshake(i18n::format(
                    "crypto.handshake_decrypt_failed",
                    &[&error.to_string()],
                ))
            })
    }

    #[cfg(test)]
    pub fn public_der(&self) -> &[u8] {
        &self.public_der
    }
}

pub(crate) fn build_enckey(public_der: &[u8]) -> Vec<u8> {
    let digest = Md5::digest(public_der);
    let base64 = base64::engine::general_purpose::STANDARD.encode(public_der);
    let encrypted = Aes256CbcEncryptor::new((&KEY_TABLE[16..48]).into(), (&KEY_TABLE[..16]).into())
        .encrypt_padded_vec_mut::<Pkcs7>(base64.as_bytes());
    let mut output = Vec::with_capacity(16 + encrypted.len());
    output.extend_from_slice(&digest);
    output.extend_from_slice(&encrypted);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enckey_has_md5_prefix_and_aes_block_body() {
        let keys = SessionKeys::generate().expect("key generation");
        let encoded = keys.build_enckey();
        assert_eq!(&encoded[..16], &Md5::digest(keys.public_der())[..]);
        assert_eq!((encoded.len() - 16) % 16, 0);
        assert!(encoded.len() > keys.public_der().len());
    }

    #[test]
    fn rsa_signature_is_128_bytes() {
        let keys = SessionKeys::generate().expect("key generation");
        assert_eq!(keys.sign(b"ssp").expect("signature").len(), 128);
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let keys = SessionKeys::generate().expect("key generation");
        let data = b"ssp";
        let mut signature = keys.sign(data).expect("signature");
        signature[0] ^= 0x01;
        let public = keys.private.to_public_key();
        assert!(
            public
                .verify(
                    Pkcs1v15Sign::new::<Sha256>(),
                    &Sha256::digest(data),
                    &signature,
                )
                .is_err()
        );
    }

    #[test]
    fn handshake_reply_accepts_mime_style_base64_line_wrapping() {
        let keys = SessionKeys::generate().expect("key generation");
        let encrypted = keys
            .private
            .to_public_key()
            .encrypt(&mut OsRng, Pkcs1v15Encrypt, b"ok")
            .expect("encrypt");
        let encoded = base64::engine::general_purpose::STANDARD.encode(encrypted);
        let wrapped = format!("{}\r\n{}\n", &encoded[..76], &encoded[76..]);
        assert_eq!(
            keys.decrypt_handshake_result(wrapped.as_bytes())
                .expect("decrypt"),
            b"ok"
        );
    }
}
