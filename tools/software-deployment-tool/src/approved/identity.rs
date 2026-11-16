use super::{policy::{Rule,TargetLease},store};
use crate::{model::{Credentials,Result},win::{self,Handle,wide}};
use std::{collections::BTreeMap,ffi::c_void,mem::{size_of,zeroed},ptr::{null,null_mut}};
use windows_sys::Win32::{Foundation::*,Security::{*,Authorization::*},System::{Threading::*,RemoteDesktop::*,Environment::*},UI::Shell::{PROFILEINFOW,LoadUserProfileW,UnloadUserProfile}};
use zeroize::Zeroizing;

pub fn token_info(token:HANDLE,kind:TOKEN_INFORMATION_CLASS)->Result<Vec<u64>>{
    unsafe{
        let mut bytes=0;GetTokenInformation(token,kind,null_mut(),0,&mut bytes);
        if bytes==0||bytes>1024*1024{return Err(win::error("TOKEN_QUERY_FAILED"));}
        let mut buffer=vec![0u64;(bytes as usize).div_ceil(8)];
        win::check(GetTokenInformation(token,kind,buffer.as_mut_ptr().cast(),bytes,&mut bytes),"TOKEN_QUERY_FAILED")?;Ok(buffer)
    }
}
pub fn sid_text(sid:PSID)->Result<String>{
    unsafe{let mut text=null_mut();win::check(ConvertSidToStringSidW(sid,&mut text),"SID_QUERY_FAILED")?;
        let result=win::string(text);LocalFree(text.cast());Ok(result)}
}
pub fn user_sid(token:HANDLE)->Result<String>{
    let data=token_info(token,TokenUser)?;
    unsafe{sid_text((*(data.as_ptr() as *const TOKEN_USER)).User.Sid)}
}
pub fn is_admin(token:HANDLE)->Result<bool>{
    unsafe{
        let mut impersonation=null_mut();
        win::check(DuplicateToken(token,SecurityImpersonation,&mut impersonation),"TOKEN_DUPLICATE_FAILED")?;
        let impersonation=Handle(impersonation);let mut sid=null_mut();
        win::check(ConvertStringSidToSidW(wide("S-1-5-32-544").as_ptr(),&mut sid),"SID_QUERY_FAILED")?;
        let mut member=0;let success=CheckTokenMembership(impersonation.0,sid,&mut member);LocalFree(sid);
        win::check(success,"ADMIN_QUERY_FAILED")?;Ok(member!=0)
    }
}
pub fn elevated(mut token:Handle)->Result<Handle>{
    unsafe{
        let original=user_sid(token.0)?;
        let mut kind=0u32;let mut size=0;
        win::check(GetTokenInformation(token.0,TokenElevationType,(&mut kind as *mut u32).cast(),4,&mut size),"TOKEN_QUERY_FAILED")?;
        if kind==TokenElevationTypeLimited as u32{
            let mut linked:TOKEN_LINKED_TOKEN=zeroed();
            win::check(GetTokenInformation(token.0,TokenLinkedToken,(&mut linked as *mut TOKEN_LINKED_TOKEN).cast(),size_of::<TOKEN_LINKED_TOKEN>() as u32,&mut size),"ACCOUNT_NOT_ELEVATED")?;
            token=Handle(linked.LinkedToken);
            if user_sid(token.0)?!=original{return Err("ACCOUNT_IDENTITY_MISMATCH".into());}
        }
        let mut elevation=0u32;
        win::check(GetTokenInformation(token.0,TokenElevation,(&mut elevation as *mut u32).cast(),4,&mut size),"TOKEN_QUERY_FAILED")?;
        if original=="S-1-5-18"||!is_admin(token.0)?||elevation==0{return Err("ACCOUNT_NOT_ELEVATED: 需要此账号的完整管理员令牌。".into());}
        Ok(token)
    }
}
fn logon_sid(token:HANDLE)->Result<Vec<u8>>{
    let buffer=token_info(token,TokenGroups)?;
    unsafe{
        let groups=&*(buffer.as_ptr() as *const TOKEN_GROUPS);
        for group in std::slice::from_raw_parts(groups.Groups.as_ptr(),groups.GroupCount as usize){
            if group.Attributes&0xc0000000==0xc0000000{
                let mut sid=vec![0;GetLengthSid(group.Sid) as usize];
                win::check(CopySid(sid.len() as u32,sid.as_mut_ptr().cast(),group.Sid),"SID_COPY_FAILED")?;return Ok(sid)
            }
        }
    }
    Err("DESKTOP_LOGON_MISMATCH: 缺少登录 SID。".into())
}

#[link(name="advapi32")]
unsafe extern "system"{
    fn LogonUserExExW(user:*const u16,domain:*const u16,password:*const u16,kind:u32,provider:u32,groups:*const c_void,
        token:*mut HANDLE,logon_sid:*mut c_void,profile:*mut c_void,profile_size:*mut c_void,quota:*mut c_void)->i32;
}
#[repr(C)]
struct ExtraGroups{count:u32,groups:[SID_AND_ATTRIBUTES;2]}
fn desktop_logon(credentials:&Credentials,session:u32)->Result<Handle>{
    crate::admin::privilege("SeTcbPrivilege")?;
    unsafe{
        let mut desktop=null_mut();win::check(WTSQueryUserToken(session,&mut desktop),"SESSION_INVALID: 无法读取交互会话令牌")?;
        let desktop=Handle(desktop);
        let mut logon=logon_sid(desktop.0)?;
        let mut local=null_mut();win::check(ConvertStringSidToSidW(wide("S-1-2-0").as_ptr(),&mut local),"SID_QUERY_FAILED")?;
        let groups=ExtraGroups{count:2,groups:[SID_AND_ATTRIBUTES{Sid:local,Attributes:7},
            SID_AND_ATTRIBUTES{Sid:logon.as_mut_ptr().cast(),Attributes:0xc0000007}]};
        let (user,domain)=win::split_account(&credentials.account);let user=wide(&user);let domain=domain.map(|d|wide(&d));
        let password=Zeroizing::new(wide(&credentials.password));let mut token=null_mut();
        let success=LogonUserExExW(user.as_ptr(),domain.as_ref().map_or(null(),|d|d.as_ptr()),password.as_ptr(),2,0,
            (&groups as *const ExtraGroups).cast(),&mut token,null_mut(),null_mut(),null_mut(),null_mut());
        let error=win::error("ACCOUNT_LOGON_FAILED");
        LocalFree(local);
        if success==0{return Err(error);}
        let token=elevated(Handle(token))?;
        let mut actual=logon_sid(token.0)?;
        if EqualSid(actual.as_mut_ptr().cast(),logon.as_mut_ptr().cast())==0{return Err("DESKTOP_LOGON_MISMATCH".into());}
        Ok(token)
    }
}
fn protect_default_dacl(token:HANDLE)->Result<()>{
    let sid=user_sid(token)?;
    unsafe{
        let mut descriptor=null_mut();
        win::check(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{sid})")).as_ptr(),1,&mut descriptor,null_mut()),"TOKEN_DACL_FAILED")?;
        let result=(||{
            let mut acl=null_mut();let mut present=0;let mut defaulted=0;
            win::check(GetSecurityDescriptorDacl(descriptor,&mut present,&mut acl,&mut defaulted),"TOKEN_DACL_FAILED")?;
            let data=TOKEN_DEFAULT_DACL{DefaultDacl:acl};
            win::check(SetTokenInformation(token,TokenDefaultDacl,(&data as *const TOKEN_DEFAULT_DACL).cast(),size_of::<TOKEN_DEFAULT_DACL>() as u32),"TOKEN_DACL_FAILED")
        })();
        LocalFree(descriptor);result
    }
}

struct Revert;
impl Drop for Revert{fn drop(&mut self){unsafe{RevertToSelf();}}}
fn verify_process_isolation(pid:u32,tid:u32,session:u32)->Result<()>{
    unsafe{
        let mut desktop=null_mut();win::check(WTSQueryUserToken(session,&mut desktop),"SESSION_INVALID")?;
        let desktop=Handle(desktop);if is_admin(desktop.0)?{return Ok(());}
        win::check(ImpersonateLoggedOnUser(desktop.0),"CALLER_INVALID")?;let _revert=Revert;
        for right in [1,2,8,0x10,0x20,0x40,0x200,0x40000,0x80000]{
            let handle=Handle(OpenProcess(right,0,pid));
            if !handle.0.is_null()||GetLastError()!=ERROR_ACCESS_DENIED{return Err("PROCESS_ISOLATION_FAILED: 桌面用户可能控制高权限进程。".into());}
        }
        for right in [1,2,8,0x10,0x20,0x100,0x200,0x40000,0x80000]{
            let handle=Handle(OpenThread(right,0,tid));
            if !handle.0.is_null()||GetLastError()!=ERROR_ACCESS_DENIED{return Err("PROCESS_ISOLATION_FAILED: 桌面用户可能控制高权限线程。".into());}
        }
        Ok(())
    }
}

pub struct Launched{process:Handle,token:Handle,profile:HANDLE,pub pid:u32}
// Each owned kernel handle is used by only the launch-monitor thread after transfer.
unsafe impl Send for Launched{}
impl Launched{
    pub fn wait(&self)->Result<u32>{
        unsafe{
            if WaitForSingleObject(self.process.0,INFINITE)!=WAIT_OBJECT_0{return Err(win::error("PROCESS_WAIT_FAILED"));}
            let mut code=0;win::check(GetExitCodeProcess(self.process.0,&mut code),"PROCESS_EXIT_FAILED")?;Ok(code)
        }
    }
}
impl Drop for Launched{fn drop(&mut self){unsafe{UnloadUserProfile(self.token.0,self.profile);}}}
pub fn launch(rule:&Rule,session:u32)->Result<Launched>{
    let circuit=store::directory().join("account-update-required");
    if circuit.exists(){return Err("ACCOUNT_UPDATE_REQUIRED: 凭据已失效，请由管理员重新配置。".into());}
    let credentials=store::credentials()?;
    let token=match desktop_logon(&credentials,session){
        Ok(token)=>token,
        Err(error)=>{
            if error.starts_with("ACCOUNT_LOGON_FAILED"){
                win::io(std::fs::write(&circuit,crate::tasks::timestamp().to_string()))?;
            }
            return Err(error)
        }
    };
    protect_default_dacl(token.0)?;
    for privilege in ["SeTcbPrivilege","SeAssignPrimaryTokenPrivilege","SeIncreaseQuotaPrivilege","SeBackupPrivilege","SeRestorePrivilege"]{crate::admin::privilege(privilege)?;}
    win::set_session(token.0,session)?;
    let mut user=wide(&win::split_account(&credentials.account).0);
    let mut profile=PROFILEINFOW{dwSize:size_of::<PROFILEINFOW>() as u32,dwFlags:1,lpUserName:user.as_mut_ptr(),..Default::default()};
    unsafe{win::check(LoadUserProfileW(token.0,&mut profile),"PROFILE_FAILED")?;}
    let result=(||{
        let mut environment=null_mut();
        unsafe{win::check(CreateEnvironmentBlock(&mut environment,token.0,0),"ENVIRONMENT_FAILED")?;}
        let mut values=BTreeMap::new();
        unsafe{
            let mut pointer=environment as *const u16;
            while *pointer!=0{
                let item=win::string(pointer);let length=item.encode_utf16().count()+1;
                if let Some((key,value))=item.split_once('='){if !key.is_empty(){values.insert(key.to_uppercase(),value.to_owned());}}
                pointer=pointer.add(length);
            }
            DestroyEnvironmentBlock(environment);
        }
        let windows=std::env::var("SystemRoot").unwrap_or("C:\\Windows".into());
        values.insert("PATH".into(),format!("{windows}\\System32;{windows}"));
        for name in ["COR_ENABLE_PROFILING","COR_PROFILER","COR_PROFILER_PATH","CORECLR_ENABLE_PROFILING","CORECLR_PROFILER","CORECLR_PROFILER_PATH",
            "DOTNET_STARTUP_HOOKS","DOTNET_ADDITIONAL_DEPS","DOTNET_SHARED_STORE","COMPLUS_VERSION","__COMPAT_LAYER"]{values.remove(name);}
        let clean=wide(&(values.iter().map(|(k,v)|format!("{k}={v}")).collect::<Vec<_>>().join("\0")+"\0"));
        let target=TargetLease::open(rule,true)?;
        let mut desktop=wide("winsta0\\default");
        let startup=STARTUPINFOW{cb:size_of::<STARTUPINFOW>() as u32,lpDesktop:desktop.as_mut_ptr(),..Default::default()};
        let mut process:PROCESS_INFORMATION=unsafe{zeroed()};
        target.verify(rule)?;
        unsafe{
            win::check(CreateProcessAsUserW(token.0,wide(&rule.target).as_ptr(),wide(&format!("\"{}\" {}",rule.target,rule.arguments)).as_mut_ptr(),
                null(),null(),0,CREATE_UNICODE_ENVIRONMENT|CREATE_SUSPENDED,clean.as_ptr().cast(),wide(&rule.working_directory).as_ptr(),&startup,&mut process),"PROCESS_START_FAILED")?;
            let handle=Handle(process.hProcess);let thread=Handle(process.hThread);
            let ready=verify_process_isolation(process.dwProcessId,process.dwThreadId,session).and_then(|_|{
                target.verify(rule)?;
                if ResumeThread(thread.0)==u32::MAX{Err(win::error("PROCESS_START_FAILED"))}else{Ok(())}
            });
            if let Err(error)=ready{TerminateProcess(handle.0,1);return Err(error);}
            Ok((handle,process.dwProcessId))
        }
    })();
    match result{
        Ok((process,pid))=>Ok(Launched{process,token,profile:profile.hProfile,pid}),
        Err(error)=>{unsafe{UnloadUserProfile(token.0,profile.hProfile);}Err(error)}
    }
}
