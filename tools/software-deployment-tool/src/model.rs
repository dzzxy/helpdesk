use serde::{Deserialize, Serialize};
use std::path::Path;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub type Result<T> = std::result::Result<T, String>;

/// Some engines return a valid receipt for a failed or partly completed operation.
/// Receiving that receipt does not make the operation successful.
pub fn outcome_state(data: &serde_json::Value) -> &'static str {
    if data["cancelled"]==true||data["state"]=="Cancelled" {"Cancelled"}
    else if data["state"] == "Unconfirmed" { "Unconfirmed" }
    else if data["succeeded"] == false||data["state"]=="Failed"||data["state"]=="PartialSuccess" {
        if data["partialSuccess"] == true||data["state"]=="PartialSuccess" { "PartialSuccess" } else { "Failed" }
    } else { "Succeeded" }
}

pub fn outcome_label(data: &serde_json::Value) -> &'static str {
    match outcome_state(data) {
        "Cancelled"=>"已取消", "Unconfirmed" => "结果待确认", "PartialSuccess" => "部分成功", "Failed" => "失败",
        _ => "成功",
    }
}
pub fn outcome_detail(data:&serde_json::Value)->String{
    let mut parts=Vec::new();
    for key in ["error","details","message"]{if let Some(value)=data[key].as_str().filter(|value|!value.is_empty()){parts.push(value.to_owned());}}
    if let Some(receipt) = shortcut_receipt(data).or_else(|| shortcut_receipt(&data["postInstall"]["authorization"])) {
        parts.push(receipt.replace("\r\n", " · "));
    }
    for (key, label) in [("localBundle", "本地诊断包"), ("reportPath", "逐文件分发报告")] {
        if let Some(path) = data[key].as_str().filter(|path| !path.is_empty()) {
            parts.push(format!("{label}：{path}"));
        }
    }
    for warning in data["postInstall"]["warnings"].as_array().into_iter().flatten().filter_map(serde_json::Value::as_str){parts.push(format!("安装后处理：{warning}"));}
    for item in data["junkDeletionResults"].as_array().into_iter().flatten() {
        if item["succeeded"] == false {
            let name = item["displayName"].as_str().unwrap_or("未知残留项");
            let error = item["error"].as_str().unwrap_or("未返回错误详情");
            parts.push(format!("{name}：{error}"));
        }
    }
    if parts.is_empty(){"已收到目标结果".into()}else{parts.join("；")}
}

pub fn shortcut_receipt(data: &serde_json::Value) -> Option<String> {
    let path = data["shortcutPath"].as_str().filter(|path| !path.trim().is_empty())?;
    let name = path.rsplit(['\\', '/']).next().filter(|name| !name.is_empty())?;
    let mut lines = vec![format!("已创建提权快捷方式：{name}"), format!("目标机路径：{path}")];
    if let Some(id) = data["ruleId"].as_str().filter(|id| !id.is_empty()) {
        lines.push(format!("授权 ID：{id}"));
    }
    Some(lines.join("\r\n"))
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum ConflictPolicy {
    #[default]
    Stop,
    Overwrite,
    Skip,
    Newer,
    DifferentHash,
    Ask,
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Credentials {
    pub account: String,
    pub password: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub operation: String,
    pub account: String,
    pub source: String,
    pub destination: String,
    pub arguments: String,
    #[serde(default)]
    pub sha256: String,
    pub session: u32,
    pub timeout: u32,
    pub existing: bool,
    #[serde(default)]
    pub conflict: ConflictPolicy,
    pub key: String,
    #[serde(default = "default_registry_view")]
    pub registry_view: u32,
    pub value: String,
    #[serde(default)]
    pub inventory: crate::inventory::Selection,
    #[serde(default)]
    pub logs: crate::logs::Options,
    #[serde(default)]
    pub elevation:crate::approved::management::Options,
    #[serde(default)]pub bcu:crate::bcu::Options,
    #[serde(default)]pub distribution:crate::distribution::Options,
    #[serde(default)]pub install:crate::install_options::Options,
}

impl Default for Request {
    fn default() -> Self {
        Self { id: new_id(), operation: "sessions".into(), account: String::new(),
            source: String::new(), destination: String::new(), arguments: String::new(), sha256: String::new(),
            session: 0, timeout: 1800, existing: false, conflict: ConflictPolicy::Stop,
            key: "HKLM\\SOFTWARE".into(), registry_view: default_registry_view(), value: "1".into(),
            inventory: crate::inventory::Selection::default(), logs: crate::logs::Options::default(),elevation:Default::default(),bcu:Default::default(),distribution:Default::default(),install:Default::default() }
    }
}

fn default_registry_view() -> u32 { 64 }

pub fn new_id() -> String {
    use rand::RngCore;
    let mut bytes = [0; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn host(s: &str) -> Result<String> {
    let s = s.trim();
    if s.is_empty() || s.len() > 253 || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err("目标请填写 IPv4 或 Windows 主机名，不含路径或端口。".into());
    }
    Ok(s.to_owned())
}

pub fn hosts(s: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for item in s.split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '，') {
        if item.is_empty() { continue; }
        let h = host(item)?;
        if !result.iter().any(|v: &String| v.eq_ignore_ascii_case(&h)) { result.push(h); }
    }
    if result.is_empty() { return Err("请输入目标电脑。".into()); }
    if result.len() > 500 { return Err("每批最多 500 台电脑。".into()); }
    Ok(result)
}

pub fn dword(s: &str) -> Result<u32> {
    let s = s.trim();
    let n = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
    } else { s.parse() };
    n.map_err(|_| "DWORD 必须是 0～4294967295 的整数或 0x 开头的十六进制。".into())
}

pub fn drive_path(path: &str) -> Result<()> {
    let b = path.as_bytes();
    if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\'
        || path.contains(['\0', '"', '/', '*', '?']) || path[2..].contains(':')
        || path.split('\\').any(|s| s == ".." || s == ".") {
        return Err("请输入完整的目标磁盘路径，例如 C:\\Temp\\App。".into());
    }
    Ok(())
}

pub fn unc(computer: &str, path: &str) -> Result<String> {
    drive_path(path)?;
    Ok(format!("\\\\{}\\{}${}", host(computer)?, &path[..1], &path[2..]))
}

impl Request {
    pub fn validate(&self) -> Result<()> {
        if self.id.len() != 32 || !self.id.bytes().all(|c| c.is_ascii_hexdigit()) { return Err("任务 ID 无效。".into()); }
        if self.account.trim().is_empty() || self.account.contains(['\0', '"']) { return Err("请输入管理员账号，建议使用 域\\账号。".into()); }
        if !(10..=86400).contains(&self.timeout) { return Err("超时须在 10～86400 秒。".into()); }
        match self.operation.as_str() {
            "sessions" | "registry-query" | "security-info" | "security-clear" | "diagnostic-logs" => (),
            "inventory"=>{if ![self.inventory.system,self.inventory.hardware,self.inventory.storage,self.inventory.network,self.inventory.sessions,self.inventory.software,self.inventory.hotfixes,self.inventory.services].into_iter().any(|selected|selected){return Err("请至少选择一项采集内容。".into());}},
            "elevation-list"|"elevation-preview"|"elevation-discover"|"elevation-create"|"elevation-approve"|"elevation-revoke"|"elevation-logs"=>(),
            "bcu-scan"|"bcu-uninstall"|"bcu-delete-junk"|"resolve-folders"=>(),
            "connection-repair"=>(),
            "registry-set" => { dword(&self.value)?; },
            "registry-browse" => {
                let k = self.key.to_uppercase();
                if !(k == "HKLM" || k.starts_with("HKLM\\") || k == "HKU" || k.starts_with("HKU\\")) || self.key.contains('\0') {
                    return Err("浏览仅支持 HKLM 或 HKU 根路径（64 位视图）。".into());
                }
            },
            "install" | "run" => {
                let ext = Path::new(&self.source).extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
                if ext != "exe" && !(self.operation == "install" && ext == "msi") { return Err("安装支持 EXE/MSI，直接运行只支持 EXE。".into()); }
                if self.existing { drive_path(&self.source)?; }
                else if !Path::new(&self.source).is_file() { return Err("本地安装文件不存在。".into()); }
                if self.arguments.contains('\0') { return Err("参数含无效字符。".into()); }
                if self.install.rollback&&(self.install.rollback_command.trim().is_empty()||self.install.rollback_command.contains(['\0','"'])||self.install.rollback_arguments.contains('\0')){return Err("启用失败回滚后，请填写有效的回滚程序和参数。".into());}
            },
            "distribute" => {
                self.distribution.validate()?;
                if self.distribution.source_computer.is_empty(){if !Path::new(&self.source).exists(){return Err("分发源不存在。".into());}}
                else{host(&self.distribution.source_computer)?;drive_path(&self.source)?;}
                if self.distribution.destination_preset=="Custom"{drive_path(&self.destination)?;}
            },
            _ => return Err("不支持的操作。".into()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]fn engine_failure_receipts_are_not_successful(){
        use serde_json::json;
        assert_eq!(outcome_state(&json!({"succeeded":false,"error":"uninstall failed"})),"Failed");
        assert_eq!(outcome_state(&json!({"succeeded":false,"partialSuccess":true})),"PartialSuccess");
        assert_eq!(outcome_state(&json!({"state":"Unconfirmed"})),"Unconfirmed");
        assert_eq!(outcome_state(&json!({"succeeded":true})),"Succeeded");
        assert_eq!(outcome_state(&json!({"state":"Failed"})),"Failed");
        assert_eq!(outcome_state(&json!({"state":"Cancelled"})),"Cancelled");
    }
    #[test]
    fn shortcut_receipt_uses_the_created_filename_and_path() {
        let data = serde_json::json!({
            "name": "Microsoft Edge",
            "shortcutPath": r"C:\Users\TEST\Desktop\Microsoft Edge（提权）.lnk",
            "ruleId": "rule-1"
        });
        let receipt = shortcut_receipt(&data).unwrap();
        assert!(receipt.starts_with("已创建提权快捷方式：Microsoft Edge（提权）.lnk"));
        assert!(receipt.contains(data["shortcutPath"].as_str().unwrap()));
        assert!(receipt.contains("授权 ID：rule-1"));
        assert!(outcome_detail(&serde_json::json!({"postInstall": {"authorization": data}})).contains("rule-1"));
        assert!(shortcut_receipt(&serde_json::json!({"ruleId": "rule-1"})).is_none());
    }
    #[test] fn input_validation() {
        assert_eq!(hosts("pc01;PC01,127.0.0.1").unwrap().len(), 2);
        for s in ["", "\\\\pc01", "pc01/x", "x & whoami", "pc01:445"] { assert!(host(s).is_err()); }
        assert_eq!(dword("0xFFFFFFFF").unwrap(), u32::MAX);
        for s in ["4294967296", "-1", "1;whoami"] { assert!(dword(s).is_err()); }
        for s in ["C:\\..\\Windows", "C:Temp", "\\\\server\\share", "C:\\a:ads"] { assert!(drive_path(s).is_err()); }
        assert_eq!(unc("pc01", "D:\\Apps\\x.exe").unwrap(), "\\\\pc01\\D$\\Apps\\x.exe");
    }
}
