use crate::model::{Credentials, Result};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::{Aead, Payload}};
use rand::{RngCore, rngs::OsRng};
use rsa::{Oaep, RsaPrivateKey, RsaPublicKey, pkcs8::{DecodePublicKey, EncodePublicKey}};
use sha2::Sha256;
use std::io::{Read, Write};
use zeroize::Zeroizing;

pub fn keypair() -> Result<(RsaPrivateKey, Vec<u8>)> {
    let key = RsaPrivateKey::new(&mut OsRng, 2048).map_err(|e| e.to_string())?;
    let public = RsaPublicKey::from(&key).to_public_key_der().map_err(|e| e.to_string())?.as_bytes().to_vec();
    Ok((key, public))
}

fn frame_write(io: &mut impl Write, data: &[u8]) -> Result<()> {
    io.write_all(&(data.len() as u32).to_le_bytes()).and_then(|_| io.write_all(data)).map_err(|e| e.to_string())
}
fn frame_read(io: &mut impl Read) -> Result<Vec<u8>> {
    let mut n = [0; 4]; io.read_exact(&mut n).map_err(|e| e.to_string())?;
    let n = u32::from_le_bytes(n) as usize;
    if n == 0 || n > 16384 { return Err("凭据通道数据长度无效。".into()); }
    let mut data = vec![0; n]; io.read_exact(&mut data).map_err(|e| e.to_string())?; Ok(data)
}
fn encrypt(key: &[u8], plain: &[u8], id: &str) -> Result<Vec<u8>> {
    let aes = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let mut nonce = [0; 12]; OsRng.fill_bytes(&mut nonce);
    let mut packet = nonce.to_vec();
    packet.extend(aes.encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad: id.as_bytes() }).map_err(|e| e.to_string())?);
    Ok(packet)
}
fn decrypt(key: &[u8], data: &[u8], id: &str) -> Result<Zeroizing<Vec<u8>>> {
    if data.len() < 28 { return Err("凭据密文不完整。".into()); }
    let aes = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    aes.decrypt(Nonce::from_slice(&data[..12]), Payload { msg: &data[12..], aad: id.as_bytes() })
        .map(Zeroizing::new).map_err(|_| "凭据通道验证失败。".into())
}
pub fn send(io: &mut (impl Read + Write), public: &[u8], id: &str, credentials: &Credentials) -> Result<()> {
    let public = RsaPublicKey::from_public_key_der(public).map_err(|e| e.to_string())?;
    let key = Zeroizing::new(rand::random::<[u8; 32]>());
    let plain = Zeroizing::new(serde_json::to_vec(credentials).map_err(|e| e.to_string())?);
    let wrapped = public.encrypt(&mut OsRng, Oaep::new::<Sha256>(), key.as_ref()).map_err(|e| e.to_string())?;
    frame_write(io, &wrapped)?; frame_write(io, &encrypt(key.as_ref(), &plain, id)?)?;
    io.flush().map_err(|e| e.to_string())?;
    let reply = decrypt(key.as_ref(), &frame_read(io)?, id)?;
    if reply.as_slice() != b"LAUNCHED" { return Err("目标机未确认管理员进程启动。".into()); }
    Ok(())
}
pub fn receive(io: &mut impl Read, private: &RsaPrivateKey, id: &str) -> Result<(Credentials, Zeroizing<Vec<u8>>)> {
    let key = Zeroizing::new(private.decrypt(Oaep::new::<Sha256>(), &frame_read(io)?).map_err(|_| "无法解密会话密钥。".to_string())?);
    let plain = decrypt(&key, &frame_read(io)?, id)?;
    let credentials = serde_json::from_slice(&plain).map_err(|_| "凭据结构无效。".to_string())?;
    Ok((credentials, key))
}
pub fn acknowledge(io: &mut impl Write, key: &[u8], id: &str) -> Result<()> {
    frame_write(io, &encrypt(key, b"LAUNCHED", id)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn envelope_is_bound_and_authenticated() {
        let key = [7; 32];
        let mut data = encrypt(&key, b"password", "task-a").unwrap();
        assert_eq!(&**decrypt(&key, &data, "task-a").unwrap(), b"password");
        assert!(decrypt(&key, &data, "task-b").is_err());
        data[15] ^= 1; assert!(decrypt(&key, &data, "task-a").is_err());
        assert!(frame_read(&mut std::io::Cursor::new(u32::MAX.to_le_bytes())).is_err());
    }
    #[test] fn rsa_wrap_roundtrip() {
        let (private, public) = keypair().unwrap();
        let public = RsaPublicKey::from_public_key_der(&public).unwrap();
        let encrypted = public.encrypt(&mut OsRng, Oaep::new::<Sha256>(), &[9; 32]).unwrap();
        assert_eq!(private.decrypt(Oaep::new::<Sha256>(), &encrypted).unwrap(), vec![9;32]);
    }
}
