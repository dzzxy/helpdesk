use serde::{Serialize,Deserialize};
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(default,rename_all="camelCase")]
pub struct Options{
    pub display_name:String,pub start_menu:bool,pub desktop:bool,pub allow_user_cancel:bool,
    pub public_desktop:bool,pub elevated_shortcut:bool,pub rollback:bool,
    pub rollback_command:String,pub rollback_arguments:String,
}
impl Default for Options{fn default()->Self{Self{display_name:String::new(),start_menu:true,desktop:true,allow_user_cancel:true,public_desktop:false,elevated_shortcut:false,rollback:false,rollback_command:String::new(),rollback_arguments:String::new()}}}
pub fn msi_arguments(package:&str,arguments:&str)->String{
    let mut words=Vec::new();let mut word=String::new();let mut quoted=false;
    for character in arguments.chars(){
        if character=='"'{quoted=!quoted;word.push(character);}
        else if character.is_whitespace()&&!quoted{if !word.is_empty(){words.push(std::mem::take(&mut word));}}
        else{word.push(character);}
    }
    if !word.is_empty(){words.push(word);}
    let has=|name:&str|words.iter().any(|part|part.split_once('=').is_some_and(|(key,_)|key.eq_ignore_ascii_case(name)));
    format!("/i \"{package}\" {arguments}{}{}",if has("ALLUSERS"){""}else{" ALLUSERS=1"},if has("MSIINSTALLPERUSER"){""}else{" MSIINSTALLPERUSER=\"\""})
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn msi_scope_does_not_override_explicit_arguments(){
        let args=msi_arguments("C:\\Temp\\a.msi","/norestart");assert!(args.contains("ALLUSERS=1"));assert!(args.contains("MSIINSTALLPERUSER=\"\""));
        let args=msi_arguments("C:\\Temp\\a.msi","ALLUSERS=2 MSIINSTALLPERUSER=1");assert!(!args.contains("ALLUSERS=1"));
        let args=msi_arguments("C:\\Temp\\a.msi",r#"NOTE="keep ALLUSERS=2 as text""#);assert!(args.contains(" ALLUSERS=1"));
    }
}
