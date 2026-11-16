use crate::{bcu, model::Result, win};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Take},
};

const MAGIC: &[u8; 16] = b"CorpRustBundle01";
const FOOTER_LEN: u64 = 40;

pub const ENGINES: [(&str, &str); 2] = [
    ("BCUninstaller.zip", bcu::ARCHIVE_HASH),
    ("CorpInstaller.BcuBridge.exe", bcu::BRIDGE_HASH),
];

pub struct Executable {
    file: File,
    core_len: u64,
    asset_lengths: Option<[u64; 3]>,
}

impl Executable {
    pub fn current() -> Result<Self> {
        let mut file = win::io(File::open(win::current_exe()?))?;
        let file_len = win::io(file.metadata())?.len();
        let plain = |file| Self { file, core_len: file_len, asset_lengths: None };
        if file_len < FOOTER_LEN { return Ok(plain(file)); }
        win::io(file.seek(SeekFrom::End(-(FOOTER_LEN as i64))))?;
        let mut footer = [0; FOOTER_LEN as usize];
        win::io(file.read_exact(&mut footer))?;
        if &footer[24..] != MAGIC { return Ok(plain(file)); }

        let lengths: [u64; 3] = std::array::from_fn(|i| {
            u64::from_le_bytes(footer[i * 8..(i + 1) * 8].try_into().unwrap())
        });
        if lengths[0] != 79_654_760 || lengths[1] != 41_356_405 || !(1..=65_536).contains(&lengths[2]) {
            return Err("内嵌卸载组件长度无效。".into());
        }
        let core_len = file_len.checked_sub(FOOTER_LEN + lengths.iter().sum::<u64>())
            .filter(|length| *length >= 64).ok_or("内嵌卸载组件超出文件范围。")?;
        Ok(Self { file, core_len, asset_lengths: Some(lengths) })
    }

    pub fn core(&mut self) -> Result<Take<&mut File>> {
        win::io(self.file.seek(SeekFrom::Start(0)))?;
        Ok((&mut self.file).take(self.core_len))
    }

    pub fn asset(&mut self, index: usize) -> Result<Option<Take<&mut File>>> {
        let Some(lengths) = self.asset_lengths else { return Ok(None); };
        let offset = self.core_len + lengths[..index].iter().sum::<u64>();
        win::io(self.file.seek(SeekFrom::Start(offset)))?;
        Ok(Some((&mut self.file).take(lengths[index])))
    }

    pub fn notice(&mut self) -> Result<String> {
        let Some(mut asset) = self.asset(2)? else {
            return win::io(std::fs::read_to_string(bcu::payload_directory()?.join("THIRD-PARTY-NOTICES.md")));
        };
        let mut notice = String::new();
        win::io(asset.read_to_string(&mut notice))?;
        if notice.trim().is_empty() { return Err("内嵌卸载组件许可为空。".into()); }
        Ok(notice)
    }

    pub fn verify(&mut self) -> Result<bool> {
        if self.asset_lengths.is_none() { return Ok(false); }
        for (index, (name, expected)) in ENGINES.iter().enumerate() {
            let mut reader = self.asset(index)?.ok_or("找不到内嵌卸载组件。")?;
            let mut digest = Sha256::new();
            win::io(std::io::copy(&mut reader, &mut digest))?;
            if format!("{:x}", digest.finalize()) != *expected {
                return Err(format!("BCU 组件校验失败：{name}"));
            }
        }
        self.notice()?;
        Ok(true)
    }
}
