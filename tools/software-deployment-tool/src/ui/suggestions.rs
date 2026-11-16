use super::*;
pub(super) const MESSAGE:u32=WM_APP+26;
#[derive(Default)]pub(super) struct Suggestions{generation:u64,host:String,query:String,loading:bool,rows:Vec<(String,bool)>}
impl Ui{
    pub(super) fn invalidate_suggestions(&mut self){
        self.suggestions=Suggestions{generation:self.suggestions.generation.wrapping_add(1),..Default::default()};
        unsafe{KillTimer(self.window,2);}
        for control in [SOURCE,DISTRIBUTION_SOURCE]{let text=self.text(control);unsafe{SendMessageW(self.handle(control),CB_RESETCONTENT,0,0);}self.set(control,&text);}
    }
    pub(super) fn load_suggestions(&mut self){
        if self.busy||!((self.page==0&&self.checked(EXISTING))||(self.page==1&&self.checked(REMOTE_SOURCE))){return;}
        let distribution=self.page==1;let control=if distribution{DISTRIBUTION_SOURCE}else{SOURCE};
        let Ok(targets)=model::hosts(&self.text(if distribution{SOURCE_COMPUTER}else{TARGET}))else{return;};if targets.len()!=1{return;}
        let query=self.text(control);let host=targets[0].clone();
        if self.suggestions.host==host&&self.suggestions.query==query&&(self.suggestions.loading||!self.suggestions.rows.is_empty()){return;}
        let account=self.text(if distribution{SOURCE_ACCOUNT}else{ACCOUNT}).trim().to_owned();let mut password=self.text(if distribution{SOURCE_PASSWORD}else{PASSWORD});
        if password.is_empty(){password=crate::credentials::read(&account).unwrap_or_default();}
        if account.is_empty()||password.is_empty(){self.set(STATUS,"读取目标机路径候选需要管理员账号和密码。");return;}
        let generation=self.suggestions.generation.wrapping_add(1);
        self.suggestions=Suggestions{generation,host:host.clone(),query:query.clone(),loading:true,rows:Vec::new()};
        unsafe{SendMessageW(self.handle(control),CB_RESETCONTENT,0,0);}self.set(control,&query);
        let window=self.window as usize;let direct=!distribution&&self.direct_run_selected();let credentials=Credentials{account,password};
        self.set(STATUS,"正在读取目标机目录和文件候选…");
        std::thread::spawn(move||{
            let result=crate::remote_browser::suggestions_mode(&host,&credentials,&query,if distribution{crate::remote_browser::Mode::AnyFile}else if direct{crate::remote_browser::Mode::Executable}else{crate::remote_browser::Mode::Installer});
            post_on(window,MESSAGE,0,serde_json::json!([generation,distribution,host,query,direct,result]).to_string());
        });
    }
    pub(super) fn show_suggestions(&mut self,text:&str){unsafe{
        let Ok((generation,distribution,host,query,direct,result))=serde_json::from_str::<(u64,bool,String,String,bool,Result<Vec<(String,bool)>>)>(text)else{return;};
        if generation!=self.suggestions.generation||host!=self.suggestions.host||query!=self.suggestions.query{return;}
        self.suggestions.loading=false;
        let control=if distribution{DISTRIBUTION_SOURCE}else{SOURCE};
        if self.page!=if distribution{1}else{0}||!self.checked(if distribution{REMOTE_SOURCE}else{EXISTING})||(!distribution&&direct!=self.direct_run_selected())||self.text(control)!=query||model::hosts(&self.text(if distribution{SOURCE_COMPUTER}else{TARGET})).ok().is_none_or(|targets|targets.len()!=1||!targets[0].eq_ignore_ascii_case(&host)){return;}
        match result{
            Err(error)=>self.set(STATUS,&error),
            Ok(rows)=>{
                self.suggestions.rows=rows;SendMessageW(self.handle(control),CB_RESETCONTENT,0,0);
                for (path,directory) in &self.suggestions.rows{
                    let text=format!("{}{}",if *directory{"[目录] "}else{""},path);SendMessageW(self.handle(control),CB_ADDSTRING,0,wide(&text).as_ptr() as isize);
                }
                self.set(control,&query);self.set(STATUS,&format!("{} 个路径候选；选择目录继续进入，选择文件回填完整路径。",self.suggestions.rows.len()));
                let focus=windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus();
                if focus==self.handle(control)||GetParent(focus)==self.handle(control){SendMessageW(self.handle(control),CB_SHOWDROPDOWN,1,0);}
            }
        }
    }}
    pub(super) fn select_suggestion(&mut self){unsafe{
        let control=if self.page==1{DISTRIBUTION_SOURCE}else{SOURCE};let index=SendMessageW(self.handle(control),CB_GETCURSEL,0,0);
        if let Some((path,directory))=self.suggestions.rows.get(index as usize).cloned(){
            self.set(control,&if directory{format!("{}\\",path.trim_end_matches('\\'))}else{path});
            if directory{self.load_suggestions();}else{SendMessageW(self.handle(control),CB_SHOWDROPDOWN,0,0);}
        }
    }}
}
