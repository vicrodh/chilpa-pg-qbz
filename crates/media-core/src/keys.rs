use crate::MediaFault;
use aes::cipher::{BlockDecryptMut, KeyIvInit, StreamCipher};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SessionKey([u8; 16]);

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ContentKey([u8; 16]);

impl SessionKey {
  pub fn derive(seed: &str, descriptor: &str) -> Result<Self, MediaFault> {
    let mut components = descriptor.split('.');
    let salt = URL_SAFE_NO_PAD.decode(components.next().unwrap_or_default())?;
    let context = URL_SAFE_NO_PAD.decode(components.next().ok_or(MediaFault::Key(
      "Invalid infos format: session infos must have at least 2 dot-separated parts",
    ))?)?;
    if seed.len() % 2 != 0 {
      return Err(MediaFault::Key("Invalid infos format: odd seed length"));
    }
    let mut material = Zeroizing::new(Vec::with_capacity(seed.len() / 2));
    for digits in seed.as_bytes().chunks_exact(2) {
      let text =
        std::str::from_utf8(digits).map_err(|_| MediaFault::Key("Invalid seed encoding"))?;
      material
        .push(u8::from_str_radix(text, 16).map_err(|_| MediaFault::Key("Invalid seed digit"))?);
    }
    let mut result = Self([0; 16]);
    hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &material)
      .expand(&context, &mut result.0)
      .map_err(|_| MediaFault::Key("HKDF expand error"))?;
    Ok(result)
  }

  pub fn open(&self, envelope: &str) -> Result<ContentKey, MediaFault> {
    let mut parts = envelope.split('.').skip(1);
    let ciphertext =
      parts.next().ok_or(MediaFault::Key("Invalid key format: missing wrapped key"))?;
    let vector = parts.next().ok_or(MediaFault::Key("Invalid key format: missing IV"))?;
    let mut scratch = Zeroizing::new(URL_SAFE_NO_PAD.decode(ciphertext)?);
    let vector = URL_SAFE_NO_PAD.decode(vector)?;
    let vector: [u8; 16] =
      vector.try_into().map_err(|_| MediaFault::Key("Invalid key format: invalid IV length"))?;
    let clear = cbc::Decryptor::<aes::Aes128>::new(&self.0.into(), &vector.into())
      .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut scratch)
      .map_err(|_| MediaFault::Key("AES decrypt error: AES-CBC unwrap failed: Unpad Error"))?;
    Ok(ContentKey(
      clear
        .try_into()
        .map_err(|_| MediaFault::Key("Invalid key format: invalid content key length"))?,
    ))
  }
}

impl ContentKey {
  pub fn from_bytes(bytes: [u8; 16]) -> Self {
    Self(bytes)
  }
  pub fn expose(&self) -> &[u8; 16] {
    &self.0
  }
  pub fn apply(&self, prefix: &[u8; 8], bytes: &mut [u8]) {
    let mut counter = [0; 16];
    counter[..8].copy_from_slice(prefix);
    ctr::Ctr128BE::<aes::Aes128>::new(&self.0.into(), &counter.into()).apply_keystream(bytes);
  }
}

#[cfg(test)]
#[path = "checks/keys_checks.rs"]
mod checks_checks;
