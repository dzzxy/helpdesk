use crate::{model::{Credentials,Request,Result},win};
use serde_json::{Value,json};
use std::{collections::{HashMap,HashSet},fs,path::{Path,PathBuf}};
use windows::{core::{HSTRING,Interface},Win32::{System::Com::{CoInitializeEx,CoCreateInstance,CLSCTX_INPROC_SERVER,COINIT_MULTITHREADED,IPersistFile,STGM_READ},UI::Shell::{IShellLinkW,ShellLink}}};
use windows_sys::Win32::System::Registry::*;
struct ComGuard(bool);
impl ComGuard{fn enter()->Self{unsafe{Self(CoInitializeEx(None,COINIT_MULTITHREADED).0>=0)}}}
impl Drop for ComGuard{fn drop(&mut self){if self.0{unsafe{windows::Win32::System::Com::CoUninitialize();}}}}

pub struct Snapshot{applications:Value,shortcut_roots:Vec<(PathBuf,bool)>}
impl Snapshot{pub fn take(credentials:&Credentials)->Self{
    let system=PathBuf::from(std::env::var("SystemRoot").unwrap_or("C:\\Windows".into())).join("System32\\config\\systemprofile");
    let mut roots=vec![(system.join("Desktop"),false),(system.join("AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs"),true)];
    for (name,fallback,programs) in [("Desktop","Desktop",false),("Programs","AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs",true)]{if let Ok(path)=win::profile_folder(&credentials.account,name,fallback){roots.push((path,programs));}}
    for (id,programs) in [(windows_sys::Win32::UI::Shell::FOLDERID_PublicDesktop,false),(windows_sys::Win32::UI::Shell::FOLDERID_CommonPrograms,true)]{if let Ok(path)=win::common_folder(&id){roots.push((path,programs));}}
    roots.sort();roots.dedup();Self{applications:crate::inventory::installed_software(),shortcut_roots:roots}
}}
fn normalized(value:&str)->String{value.to_lowercase().chars().filter(|c|c.is_alphanumeric()).collect()}
fn same_path(left:&Path,right:&Path)->bool{
    match (fs::canonicalize(left),fs::canonicalize(right)){(Ok(left),Ok(right))=>left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy()),_=>left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy())}
}
fn requested(request:&Request)->String{
    if !request.install.display_name.is_empty(){return normalized(&request.install.display_name);}
    let name=Path::new(&request.source).file_stem().and_then(|s|s.to_str()).unwrap_or_default();
    normalized(name.split(['_','-']).next().unwrap_or(name)).trim_end_matches(|c:char|c.is_ascii_digit()||c=='.').trim_end_matches("setup").trim_end_matches("installer").to_owned()
}
fn icon_target(value:&str)->Option<PathBuf>{
    let value=value.trim();
    let value=if let Some(rest)=value.strip_prefix('"'){rest.split('"').next()?}else{value.rsplit_once(',').filter(|(_,index)|index.trim().parse::<i32>().is_ok()).map(|(path,_)|path).unwrap_or(value)};
    let value=win::expand_environment(value).ok()?;let path=PathBuf::from(value);
    (path.extension().is_some_and(|ext|ext.eq_ignore_ascii_case("exe"))&&path.is_file()).then_some(path)
}
fn executable(path:&Path)->bool{
    path.extension().is_some_and(|extension|extension.eq_ignore_ascii_case("exe"))&&path.file_stem().is_some_and(|name|{
        let name=name.to_string_lossy().to_lowercase();!["unins","uninstall","setup","update","crash","report","repair","helper","service","elevat"].iter().any(|part|name.contains(part))
    })
}
fn product_tokens(request:&Request)->Vec<String>{
    let source=if !request.install.display_name.is_empty(){request.install.display_name.clone()}else{Path::new(&request.source).file_stem().unwrap_or_default().to_string_lossy().into_owned()};
    let mut camel=String::new();let mut previous=' ';
    for character in source.chars(){if previous.is_ascii_lowercase()&&character.is_ascii_uppercase(){camel.push(' ');}camel.push(character);previous=character;}
    let mut tokens=Vec::new();
    for text in [&source,&camel]{
        let mut token=String::new();let mut kind=0;
        for character in text.chars().chain(std::iter::once(' ')){
            let next=if character.is_ascii_alphabetic(){1}else if ('\u{4e00}'..='\u{9fff}').contains(&character){2}else{0};
            if next!=kind||next==0{
                if token.chars().count()>=if kind==1{3}else{2}{let token=token.to_lowercase();if !["setup","installer","install","standalone","windows","win","exe","msi"].contains(&token.as_str()){tokens.push(token);}}
                token.clear();kind=next;
            }
            if next!=0{token.push(character);}
        }
    }
    tokens.sort();tokens.dedup();tokens
}
fn matches_product(value:&str,tokens:&[String])->bool{let value=normalized(value);tokens.iter().any(|token|value.contains(token))}
fn score_executable(path:&Path,root:&Path,display:&str,expected:&str)->i32{
    if !executable(path){return -1;}
    let name=normalized(&path.file_stem().unwrap_or_default().to_string_lossy());
    let related=|token:&str|!token.is_empty()&&(name.contains(token)||token.contains(&name));
    (if path.parent()==Some(root){35}else{0})
        +(if ["launcher","player","client","main"].iter().any(|word|name.contains(word)){25}else{0})
        +(if related(&normalized(display)){80}else{0})+(if related(expected){60}else{0})
}
fn location_target(directory:&Path,display:&str,expected:&str)->Option<(PathBuf,i32)>{
    let mut roots=vec![directory.to_owned()];
    for entry in fs::read_dir(directory).into_iter().flatten().flatten().take(2000){
        use std::os::windows::fs::MetadataExt;
        if let Ok(metadata)=fs::symlink_metadata(entry.path()){if metadata.is_dir()&&metadata.file_attributes()&0x400==0&&roots.len()<41{roots.push(entry.path());}}
    }
    let mut candidates=Vec::new();
    for root in roots{for entry in fs::read_dir(root).into_iter().flatten().flatten().take(2000){
        use std::os::windows::fs::MetadataExt;
        let Ok(metadata)=fs::symlink_metadata(entry.path())else{continue;};if !metadata.is_file()||metadata.file_attributes()&0x400!=0{continue;}
        let score=score_executable(&entry.path(),directory,display,expected);if score>=70{candidates.push((entry.path(),score));}
    }}
    candidates.sort_by_key(|(_,score)|-*score);
    if candidates.len()>1&&candidates[0].1==candidates[1].1{None}else{candidates.into_iter().next()}
}
fn discover(snapshot:&Snapshot,request:&Request)->Result<(String,PathBuf)>{
    let before=snapshot.applications.as_array().into_iter().flatten().map(|entry|format!("{}:{}",entry["registryPath"],entry["architecture"])).collect::<HashSet<_>>();
    let expected=requested(request);let tokens=product_tokens(request);if tokens.is_empty(){return Err("软件名称无法自动识别，请填写软件名称后再安装，或在软件提权页选择实际程序。".into());}
    let applications=crate::inventory::installed_software();let mut candidates=HashMap::<String,(String,PathBuf,i32)>::new();
    for entry in applications.as_array().into_iter().flatten(){
        let name=entry["displayName"].as_str().unwrap_or_default();let key=format!("{}:{}",entry["registryPath"],entry["architecture"]);let matched=matches_product(name,&tokens);
        if before.contains(&key)&&!matched{continue;}
        let resolved=entry["displayIcon"].as_str().and_then(icon_target).filter(|path|executable(path)).map(|path|(path,200))
            .or_else(||entry["installLocation"].as_str().filter(|path|!path.trim().is_empty()).and_then(|path|location_target(Path::new(path),name,&expected)));
        let Some((path,score))=resolved else{continue;};
        if !matched&&!matches_product(&path.file_stem().unwrap_or_default().to_string_lossy(),&tokens){continue;}
        let score=score+if name.eq_ignore_ascii_case(&request.install.display_name){100}else if matched{70}else{0};
        let key=path.to_string_lossy().to_lowercase();if candidates.get(&key).is_none_or(|(_,_,previous)|*previous<score){candidates.insert(key,(name.into(),path,score));}
    }
    let mut candidates=candidates.into_values().collect::<Vec<_>>();
    let exact=candidates.iter().filter(|(name,_,_)|name.eq_ignore_ascii_case(&request.install.display_name)).collect::<Vec<_>>();
    if let [(name,path,_)]=exact.as_slice(){return Ok((name.clone(),path.clone()));}
    candidates.sort_by_key(|(_,_,score)|-*score);
    if candidates.is_empty()||candidates.len()>1&&(candidates[0].2<70||candidates[0].2==candidates[1].2){return Err(format!("无法唯一确定本次软件的启动 EXE（候选数：{}），未创建快捷方式。",candidates.len()));}
    let (name,path,_)=candidates.remove(0);Ok((name,path))
}
pub(crate) fn user_folder(account:&str,name:&str,fallback:&str)->Result<PathBuf>{
    win::profile_folder(account,name,fallback)
}
fn read_link(path:&Path)->Result<(PathBuf,String)>{unsafe{
    let _com=ComGuard::enter();
    let link:IShellLinkW=CoCreateInstance(&ShellLink,None,CLSCTX_INPROC_SERVER).map_err(|error|error.to_string())?;let file:IPersistFile=link.cast().map_err(|error|error.to_string())?;
    file.Load(&HSTRING::from(path.to_string_lossy().as_ref()),STGM_READ).map_err(|error|error.to_string())?;
    let mut target=vec![0u16;32768];let mut arguments=vec![0u16;32768];link.GetPath(&mut target,std::ptr::null_mut(),0).map_err(|error|error.to_string())?;link.GetArguments(&mut arguments).map_err(|error|error.to_string())?;
    Ok((String::from_utf16_lossy(&target).trim_end_matches('\0').into(),String::from_utf16_lossy(&arguments).trim_end_matches('\0').into()))
}}
fn private_target(path:&Path,credentials:&Credentials,user:&str)->bool{
    let lower=fs::canonicalize(path).unwrap_or_else(|_|path.to_owned()).to_string_lossy().to_lowercase();
    let system=PathBuf::from(std::env::var("SystemRoot").unwrap_or("C:\\Windows".into()));
    let mut private=vec![system.join("System32\\config\\systemprofile"),system.join("SysWOW64\\config\\systemprofile")];
    if !win::account_sid(user).ok().zip(win::account_sid(&credentials.account).ok()).is_some_and(|(user,admin)|user==admin){
        if let Ok(sid)=win::account_sid(&credentials.account){if let Ok(profile)=win::reg_string(HKEY_LOCAL_MACHINE,&format!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList\\{sid}"),"ProfileImagePath"){if let Ok(profile)=win::expand_environment(&profile){private.push(profile.into());}}}
    }
    private.into_iter().any(|root|lower.starts_with(&format!("{}\\",fs::canonicalize(&root).unwrap_or(root).to_string_lossy().trim_end_matches('\\').to_lowercase())))
}
fn link_files(directory:&Path,depth:usize,files:&mut Vec<PathBuf>){
    if depth==0||files.len()>=3000{return;}
    for entry in fs::read_dir(directory).into_iter().flatten().flatten().take(5000){
        use std::os::windows::fs::MetadataExt;
        let Ok(metadata)=fs::symlink_metadata(entry.path())else{continue;};if metadata.file_attributes()&0x400!=0{continue;}
        if metadata.is_dir(){link_files(&entry.path(),depth-1,files);}else if entry.path().extension().is_some_and(|extension|extension.eq_ignore_ascii_case("lnk")){files.push(entry.path());}
        if files.len()>=3000{break;}
    }
}
fn copy_link(source:&Path,destination:&Path)->Result<()>{
    if destination.exists(){let existing=read_link(destination)?;let expected=read_link(source)?;if same_path(&existing.0,&expected.0)&&existing.1==expected.1{return Ok(());}return Err(format!("已有同名快捷方式，未覆盖：{}",destination.display()));}
    win::io(fs::create_dir_all(destination.parent().ok_or("快捷方式目录无效。")?))?;
    let temporary=destination.with_file_name(format!(".corprust-{}.lnk",crate::model::new_id()));
    struct Temporary(PathBuf);impl Drop for Temporary{fn drop(&mut self){let _=fs::remove_file(&self.0);}}let _temporary=Temporary(temporary.clone());
    win::io(fs::copy(source,&temporary))?;
    unsafe{win::check(windows_sys::Win32::Storage::FileSystem::MoveFileExW(win::wide(&temporary.to_string_lossy()).as_ptr(),win::wide(&destination.to_string_lossy()).as_ptr(),windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH),"提交快捷方式失败，未覆盖已有文件")?;}
    Ok(())
}
pub fn shortcut(path:&Path,target:&Path,arguments:&str)->Result<bool>{unsafe{
    let _com=ComGuard::enter();
    let link:IShellLinkW=CoCreateInstance(&ShellLink,None,CLSCTX_INPROC_SERVER).map_err(|e|e.to_string())?;
    let file:IPersistFile=link.cast().map_err(|e|e.to_string())?;
    if path.exists(){
        file.Load(&HSTRING::from(path.to_string_lossy().as_ref()),STGM_READ).map_err(|e|e.to_string())?;
        let mut existing=vec![0u16;32768];let mut args=vec![0u16;32768];
        link.GetPath(&mut existing,std::ptr::null_mut(),0).map_err(|e|e.to_string())?;link.GetArguments(&mut args).map_err(|e|e.to_string())?;
        let existing=String::from_utf16_lossy(&existing).trim_end_matches('\0').to_owned();let args=String::from_utf16_lossy(&args).trim_end_matches('\0').to_owned();
        return if same_path(Path::new(&existing),target)&&args==arguments{Ok(false)}else{Err(format!("已有同名快捷方式，未覆盖：{}",path.display()))};
    }
    win::io(fs::create_dir_all(path.parent().ok_or("快捷方式目录无效。")?))?;
    link.SetPath(&HSTRING::from(target.to_string_lossy().as_ref())).map_err(|e|e.to_string())?;link.SetArguments(&HSTRING::from(arguments)).map_err(|e|e.to_string())?;
    link.SetWorkingDirectory(&HSTRING::from(target.parent().ok_or("程序目录无效。")?.to_string_lossy().as_ref())).map_err(|e|e.to_string())?;link.SetIconLocation(&HSTRING::from(target.to_string_lossy().as_ref()),0).map_err(|e|e.to_string())?;
    file.Save(&HSTRING::from(path.to_string_lossy().as_ref()),false).map_err(|e|e.to_string())?;
    windows_sys::Win32::UI::Shell::SHChangeNotify(0x2,0x5,win::wide(&path.to_string_lossy()).as_ptr().cast(),std::ptr::null());Ok(true)
}}
pub fn complete(snapshot:&Snapshot,request:&Request,credentials:&Credentials,user:&str,directory:&Path)->Value{
    let outcome=(||->Result<Value>{
        let mut paths=Vec::new();let mut warnings=Vec::new();let mut desktop_ready=false;let mut programs_ready=false;
        let desktop=if request.install.public_desktop{win::common_folder(&windows_sys::Win32::UI::Shell::FOLDERID_PublicDesktop)}else{user_folder(user,"Desktop","Desktop")};
        let programs=user_folder(user,"Programs","AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs");let tokens=product_tokens(request);
        let mut links=Vec::new();for (root,programs) in &snapshot.shortcut_roots{let mut files=Vec::new();link_files(root,6,&mut files);links.extend(files.into_iter().map(|path|(path,*programs)));}links.sort();links.dedup();
        for (link,is_program) in links{
            if directory.join("cancel.signal").exists(){return Ok(json!({"succeeded":false,"cancelled":true,"shortcuts":paths,"warnings":["安装器已完成，安装后处理已取消。"]}));}
            let Ok((target,_))=read_link(&link)else{continue;};
            if !matches_product(&link.file_stem().unwrap_or_default().to_string_lossy(),&tokens)&&!matches_product(&target.file_stem().unwrap_or_default().to_string_lossy(),&tokens){continue;}
            if !executable(&target)||crate::model::drive_path(&target.to_string_lossy()).is_err()||!target.is_file(){continue;}
            if private_target(&target,credentials,user){warnings.push(format!("快捷方式指向执行账号个人目录，未复制：{}",target.display()));continue;}
            for (enabled,directory,desktop_kind) in [(request.install.desktop&&!request.install.elevated_shortcut,&desktop,true),(request.install.start_menu&&is_program,&programs,false)]{
                if !enabled{continue;}
                let result=directory.as_ref().map_err(Clone::clone).and_then(|directory|{let path=directory.join(link.file_name().ok_or("快捷方式名称无效。")?);copy_link(&link,&path).map(|_|path)});
                match result{Ok(path)=>{if desktop_kind{desktop_ready=true;}else{programs_ready=true;}paths.push(path);},Err(error)=>warnings.push(error)}
            }
        }
        if (!request.install.desktop||request.install.elevated_shortcut||desktop_ready)&&(!request.install.start_menu||programs_ready)&&!request.install.elevated_shortcut{
            paths.sort();paths.dedup();return Ok(json!({"succeeded":warnings.is_empty(),"shortcuts":paths,"warnings":warnings}));
        }
        let (name,target)=match discover(snapshot,request){Ok(result)=>result,Err(error)=>{warnings.push(error);return Ok(json!({"succeeded":false,"shortcuts":paths,"warnings":warnings}));}};
        if private_target(&target,credentials,user){warnings.push("软件安装到执行账号个人目录，目标用户可能无法使用。请使用所有用户安装。".into());return Ok(json!({"succeeded":false,"shortcuts":paths,"warnings":warnings}));}
        let safe=name.chars().map(|c|if "<>:\"/\\|?*".contains(c)||c.is_control(){'_'}else{c}).collect::<String>();
        for (enabled,directory) in [(request.install.start_menu&&!programs_ready,programs),(request.install.desktop&&!request.install.elevated_shortcut&&!desktop_ready,desktop)]{
            if !enabled{continue;}
            match directory.and_then(|directory|{let path=directory.join(format!("{safe}.lnk"));shortcut(&path,&target,"").map(|_|path)}){Ok(path)=>paths.push(path),Err(error)=>warnings.push(error)}
        }
        let mut authorization=Value::Null;
        if request.install.elevated_shortcut{
            if directory.join("cancel.signal").exists(){return Ok(json!({"succeeded":false,"cancelled":true,"shortcuts":paths,"warnings":["安装已完成，已取消剩余授权操作。"]}));}
            let mut grant=request.clone();grant.operation="elevation-create".into();grant.source=target.to_string_lossy().into();grant.elevation.trusted=true;grant.elevation.user_account=user.into();
            match crate::approved::management::execute(&grant,credentials){Ok(value)=>authorization=value,Err(error)=>warnings.push(format!("授权入口未创建：{error}"))}
        }
        Ok(json!({"succeeded":warnings.is_empty(),"application":name,"target":target,"shortcuts":paths,"authorization":authorization,"warnings":warnings}))
    })();
    outcome.unwrap_or_else(|error|json!({"succeeded":false,"warnings":[error]}))
}
#[cfg(test)]mod tests{use super::*;#[test]fn product_name_and_icon_parsing_do_not_select_uninstallers(){
    let mut request=Request::default();request.operation="install".into();request.account="DOMAIN\\admin".into();request.source="C:\\MuMu_6.0.4_test.exe".into();assert_eq!(requested(&request),"mumu");assert!(executable(Path::new("C:\\App\\App.exe")));assert!(!executable(Path::new("C:\\App\\Uninstall.exe")));
    request.install.display_name="LobsterAIDev".into();assert!(matches_product("LobsterAI",&product_tokens(&request)));assert!(!matches_product("Unrelated Tool",&product_tokens(&request)));
}
    struct Fixture(PathBuf);impl Fixture{fn new()->Self{let path=std::env::temp_dir().join(format!("CorpRustPostInstallTest-{}",crate::model::new_id()));fs::create_dir(&path).unwrap();Self(path)}}
    impl Drop for Fixture{fn drop(&mut self){if self.0.parent()==Some(std::env::temp_dir().as_path())&&self.0.file_name().is_some_and(|name|name.to_string_lossy().starts_with("CorpRustPostInstallTest-")){let _=fs::remove_dir_all(&self.0);}}}
    #[test]fn executable_discovery_refuses_tied_candidates(){
        let fixture=Fixture::new();fs::write(fixture.0.join("Product.exe"),b"fixture only").unwrap();fs::write(fixture.0.join("Product64.exe"),b"fixture only").unwrap();
        assert!(location_target(&fixture.0,"Product","product").is_none());fs::write(fixture.0.join("ProductClient.exe"),b"fixture only").unwrap();
        assert_eq!(location_target(&fixture.0,"Product","product").unwrap().0.file_name().unwrap(),"ProductClient.exe");
    }
    #[test]fn shortcuts_are_idempotent_and_preserve_conflicting_entries(){
        let fixture=Fixture::new();let target=fixture.0.join("Product.exe");let other=fixture.0.join("Other.exe");let link=fixture.0.join("Product.lnk");fs::write(&target,b"fixture only").unwrap();fs::write(&other,b"fixture only").unwrap();
        assert!(shortcut(&link,&target,"--fixed").unwrap());let before=fs::read(&link).unwrap();assert!(!shortcut(&link,&target,"--fixed").unwrap());
        assert!(shortcut(&link,&other,"--fixed").is_err());assert_eq!(fs::read(&link).unwrap(),before);
    }
    #[test]fn migrated_shortcuts_keep_installer_arguments_and_do_not_replace_other_targets(){
        let fixture=Fixture::new();let target=fixture.0.join("Product.exe");let source=fixture.0.join("Installer.lnk");let destination=fixture.0.join("UserDesktop\\Product.lnk");fs::write(&target,b"fixture only").unwrap();
        shortcut(&source,&target,"--profile \"User Name\"").unwrap();copy_link(&source,&destination).unwrap();let (copied,args)=read_link(&destination).unwrap();
        assert!(same_path(&copied,&target));assert_eq!(args,"--profile \"User Name\"");copy_link(&source,&destination).unwrap();
        let other=fixture.0.join("Other.exe");fs::write(&other,b"fixture only").unwrap();let conflict=fixture.0.join("Other.lnk");shortcut(&conflict,&other,"").unwrap();let before=fs::read(&destination).unwrap();
        assert!(copy_link(&conflict,&destination).is_err());assert_eq!(fs::read(&destination).unwrap(),before);
    }
}
