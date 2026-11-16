#![windows_subsystem = "windows"]
mod admin;
mod inventory;
mod export;
mod tasks;
mod logs;
mod approved;
mod bcu;
mod bundle;
mod bcu_junk;
mod bcu_selection;
mod repair;
mod credentials;
mod crypto;
mod model;
mod remote;
mod ui;
mod form;
mod distribution;
mod install_options;
mod install_wizard;
mod postinstall;
mod remote_browser;
mod diagnostics;
mod access_probe;
mod win;
mod worker;

fn main() {
    if std::env::var_os("CORP_RUST_ASKPASS_PIPE").is_some(){
        if repair::askpass().is_err(){std::process::exit(1);}
        return;
    }
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("--access-probe")=>args.get(2).ok_or("缺少目标电脑。".into()).and_then(|host|access_probe::child(host,args.get(3).is_none_or(|arg|arg!="--rpc-only"),args.get(3).is_some_and(|arg|arg=="--wmi-only"))),
        #[cfg(debug_assertions)]
        Some("--preview-bcu-junk") => {
            unsafe{windows_sys::Win32::UI::Controls::InitCommonControls();}
            let items=[("VeryGood",true),("Good",true),("Questionable",true),("Bad",false),("Unknown",true)].into_iter().enumerate().map(|(index,(confidence,supported))|{
                let mut item=bcu_junk::Item{id:String::new(),kind:"RegistryKey".into(),path:format!("HKLM\\Software\\CorpRustUiFixture\\Item{index}"),value_name:None,display_name:format!("界面测试项目 {index}"),confidence:confidence.into(),supported};
                item.id=item.identity();serde_json::to_value(item).unwrap()
            }).collect::<Vec<_>>();
            bcu_selection::select(std::ptr::null_mut(),"UI fixture (no remote target)",&items).map(|_|())
        },
        Some("--approved-service") => approved::service::run(),
        Some("--bcu-host") => bcu::host(&args),
        Some("--launch-rule") => args.get(2).ok_or("缺少授权编号。".into()).and_then(|id|{
            approved::policy::id(id)?;
            let mut request=approved::service::Request::default();
            request.operation="launch".into();request.rule_id=Some(id.clone());
            approved::service::send(&request).map(|_|())
        }),
        Some("--service") => worker::service(&args),
        Some("--broker") => worker::broker(&args),
        Some("--self-test") => {
            let result = worker::self_test();
            let text = match &result { Ok(x) => x.clone(), Err(e) => e.clone() };
            if let Some(path) = args.get(2) { let _ = std::fs::write(path, text); }
            result.map(|_| ())
        },
        _ => ui::run(),
    };
    if let Err(error) = result {
        if args.get(1).is_some_and(|arg|arg=="--access-probe"){use std::io::Write;let _=std::io::stderr().write_all(error.as_bytes());}
        if args.len() == 1 || args.get(1).is_some_and(|arg|arg=="--launch-rule") { win::message("企业软件部署", &error); }
        std::process::exit(1);
    }
}
