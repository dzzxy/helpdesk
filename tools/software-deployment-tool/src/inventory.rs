use crate::{model::Result, win};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use wmi::{Variant, WMIConnection};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Selection {
    pub system: bool,
    pub hardware: bool,
    pub storage: bool,
    pub network: bool,
    pub sessions: bool,
    pub software: bool,
    pub hotfixes: bool,
    pub services: bool,
}

impl Default for Selection {
    fn default() -> Self {
        Self { system: true, hardware: true, storage: true, network: true,
            sessions: true, software: true, hotfixes: true, services: true }
    }
}

pub fn query(connection: &WMIConnection, statement: &str) -> Result<Value> {
    let rows: Vec<HashMap<String, Variant>> = connection.raw_query(statement).map_err(|error|error.to_string())?;
    serde_json::to_value(rows).map_err(|error|error.to_string())
}

pub fn collect(selection: &Selection) -> Result<Value> {
    let connection = WMIConnection::new().map_err(|error|error.to_string())?;
    let mut output = json!({
        "computer": std::env::var("COMPUTERNAME").unwrap_or_default(),
        "collectedAt": crate::tasks::timestamp(),
        "warnings": []
    });
    let groups = [
        (selection.system, "system", "SELECT Manufacturer,Model,SystemType,TotalPhysicalMemory,UserName,Domain,PartOfDomain FROM Win32_ComputerSystem"),
        (selection.system, "operatingSystem", "SELECT Caption,Version,BuildNumber,OSArchitecture,SerialNumber,InstallDate,LastBootUpTime,WindowsDirectory,SystemDirectory,Locale,LocalDateTime,CurrentTimeZone FROM Win32_OperatingSystem"),
        (selection.hardware, "bios", "SELECT Manufacturer,Name,SerialNumber,SMBIOSBIOSVersion,ReleaseDate FROM Win32_BIOS"),
        (selection.hardware, "baseboard", "SELECT Manufacturer,Product,SerialNumber,Version FROM Win32_BaseBoard"),
        (selection.hardware, "processors", "SELECT DeviceID,Name,Manufacturer,NumberOfCores,NumberOfLogicalProcessors,MaxClockSpeed,Architecture,ProcessorId FROM Win32_Processor"),
        (selection.hardware, "memoryModules", "SELECT DeviceLocator,BankLabel,Capacity,Speed,Manufacturer,PartNumber,SerialNumber FROM Win32_PhysicalMemory"),
        (selection.hardware, "videoControllers", "SELECT Name,VideoProcessor,DriverVersion,AdapterRAM,Status FROM Win32_VideoController"),
        (selection.storage, "physicalDisks", "SELECT DeviceID,Model,SerialNumber,InterfaceType,MediaType,Size,Status FROM Win32_DiskDrive"),
        (selection.storage, "logicalDisks", "SELECT DeviceID,VolumeName,FileSystem,DriveType,Size,FreeSpace FROM Win32_LogicalDisk"),
        (selection.network, "networkAdapters", "SELECT Description,MACAddress,DHCPEnabled,IPAddress,IPSubnet,DefaultIPGateway,DNSServerSearchOrder,SettingID FROM Win32_NetworkAdapterConfiguration WHERE IPEnabled = TRUE"),
        (selection.hotfixes, "hotfixes", "SELECT HotFixID,Description,InstalledBy,InstalledOn FROM Win32_QuickFixEngineering"),
        (selection.services, "services", "SELECT Name,DisplayName,State,StartMode,StartName,PathName FROM Win32_Service"),
    ];
    for (enabled, name, statement) in groups {
        if !enabled { continue; }
        match query(&connection, statement) {
            Ok(rows) => output[name] = rows,
            Err(error) => output["warnings"].as_array_mut().unwrap().push(json!(format!("{name}: {error}"))),
        }
    }
    if selection.sessions { output["sessions"] = win::sessions(None)?; }
    if selection.software { output["installedSoftware"] = installed_software(); }
    Ok(output)
}

pub fn installed_software() -> Value {
    let mut applications = Vec::new();
    for (root, prefix) in [("HKLM", String::new()), ("HKU", String::new())] {
        let roots = if root == "HKLM" { vec![root.to_owned()] } else {
            win::registry_browse(root,64).ok().into_iter()
                .flat_map(|data| data["subkeys"].as_array().cloned().unwrap_or_default())
                .filter_map(|sid| sid.as_str().map(str::to_owned))
                .filter(|sid| sid.starts_with("S-1-5-21-") && !sid.ends_with("_Classes"))
                .map(|sid|format!("HKU\\{sid}")).collect()
        };
        for hive in roots {
            let path = format!("{hive}\\{prefix}SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
            for view in [64,32] {
                let Ok(list) = win::registry_browse(&path, view) else { continue; };
                for subkey in list["subkeys"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    let full = format!("{path}\\{subkey}");
                    let Ok(data) = win::registry_browse(&full, view) else { continue; };
                    let values: HashMap<String,String> = data["values"].as_array().into_iter().flatten()
                        .filter_map(|value|Some((value["name"].as_str()?.into(),value["data"].as_str()?.into()))).collect();
                    let Some(name) = values.get("DisplayName").filter(|name|!name.trim().is_empty()) else { continue; };
                    applications.push(json!({
                        "displayName":name,"version":values.get("DisplayVersion"),"publisher":values.get("Publisher"),
                        "installDate":values.get("InstallDate"),"installLocation":values.get("InstallLocation"),"displayIcon":values.get("DisplayIcon"),
                        "registryPath":full,"architecture":view,"scope":hive,
                        "uninstallString":values.get("UninstallString"),"quietUninstallString":values.get("QuietUninstallString"),
                        "systemComponent":values.get("SystemComponent").is_some_and(|v|v=="1"),
                        "estimatedSizeKb":values.get("EstimatedSize").and_then(|value|value.parse::<u64>().ok())
                    }));
                }
            }
        }
    }
    applications.sort_by_key(|app|app["displayName"].as_str().unwrap_or_default().to_lowercase());
    json!(applications)
}
