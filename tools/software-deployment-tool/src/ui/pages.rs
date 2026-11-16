use super::*;
use std::path::PathBuf;
pub(super) const LOCAL_SOURCE:i32=700;
pub(super) const REMOTE_SOURCE:i32=701;
pub(super) const SOURCE_COMPUTER:i32=702;
pub(super) const SOURCE_ACCOUNT:i32=703;
pub(super) const SOURCE_PASSWORD:i32=704;
pub(super) const REMEMBER_SOURCE:i32=705;
pub(super) const DESTINATION_PRESET:i32=706;
pub(super) const TARGET_CONCURRENCY:i32=707;
pub(super) const FILE_CONCURRENCY:i32=708;
pub(super) const VERIFICATION:i32=709;
pub(super) const FILTERS:i32=710;
pub(super) const RETRY_DISTRIBUTION:i32=712;
pub(super) const LOG_CONCURRENCY:i32=713;
pub(super) const DETAIL_TABS:i32=714;
pub(super) const DETAIL_TABLE:i32=715;
pub(super) const EXPORT_JSON:i32=716;
pub(super) const EXPORT_CSV:i32=717;
pub(super) const EXPORT_EXCEL:i32=718;
pub(super) const EXPORT_SCOPE:i32=719;
pub(super) const REPAIR_NETWORK:i32=720;
pub(super) const DETAIL_OVERVIEW:i32=724;
pub(super) const BCU_LICENSE:i32=721;
pub(super) const CLEAR_DIAGNOSTICS:i32=723;
pub(super) const OPEN_BUNDLE:i32=725;
pub(super) const OPEN_BUNDLE_FOLDER:i32=726;
pub(super) const DISPLAY_NAME:i32=730;
pub(super) const MACHINE_WIDE:i32=731;
pub(super) const START_MENU:i32=732;
pub(super) const DESKTOP_SHORTCUT:i32=733;
pub(super) const ALLOW_USER_CANCEL:i32=734;
pub(super) const PUBLIC_DESKTOP:i32=735;
pub(super) const ELEVATED_SHORTCUT:i32=736;
pub(super) const ROLLBACK_ENABLED:i32=737;
pub(super) const ROLLBACK_COMMAND:i32=738;
pub(super) const ROLLBACK_ARGUMENTS:i32=739;
pub(super) const UPLOAD_PACKAGE:i32=740;
pub(super) const ELEVATION_BROWSE:i32=743;
pub(super) const ELEVATION_ADVANCED:i32=744;
pub(super) const ELEVATION_MANAGE:i32=745;
pub(super) const ELEVATION_DISCOVER:i32=746;
pub(super) const ELEVATION_FILTER:i32=747;
pub(super) const ELEVATION_CANDIDATE:i32=748;
pub(super) const ELEVATION_PREVIEW:i32=749;
pub(super) const ELEVATION_LIST:i32=750;
pub(super) const ELEVATION_RECEIPT:i32=753;
pub(super) const PRESETS:[&str;7]=["Custom","PublicDesktop","PublicStartMenu","ProgramDataCompany","UserDesktop","UserDocuments","UserDownloads"];
pub(super) struct State{pub filters:crate::distribution::Options,pub remember:[bool;PAGE_COUNT],pub last_distribution:Option<Request>,pub timeouts:[String;PAGE_COUNT],pub export_scope:[bool;8],pub sort:[Option<(usize,bool)>;PAGE_COUNT],pub scroll:[Cell<i32>;PAGE_COUNT],pub split_offset:[Cell<i32>;PAGE_COUNT],pub panes:Cell<Option<splitter::Panes>>}
impl Default for State{fn default()->Self{let mut remember=[false;PAGE_COUNT];remember[10]=true;Self{filters:Default::default(),remember,last_distribution:None,timeouts:std::array::from_fn(|page|match page{0|7=>"60",3=>"10",5=>"5",9=>"15",_=>"10"}.into()),export_scope:[true;8],sort:[None;PAGE_COUNT],scroll:std::array::from_fn(|_|Cell::new(0)),split_offset:Default::default(),panes:Cell::new(None)}}}
impl Ui{
    pub(super) fn open_bundle(&self,parent:bool){
        let Some(row)=self.selected_record()else{self.set(STATUS,"请选择已经完成的诊断任务。");return;};
        let Some(path)=row["data"]["localBundle"].as_str().map(PathBuf::from)else{self.set(STATUS,"选中任务没有可用的本地诊断包。");return;};
        if !path.is_file(){self.set(STATUS,"诊断包不存在，请检查保存目录。");return;}
        let result=if parent{std::process::Command::new("explorer.exe").arg(path.parent().unwrap()).spawn()}else{std::process::Command::new("explorer.exe").arg(path).spawn()};
        if let Err(error)=result{self.set(STATUS,&error.to_string());}
    }
    pub(super) fn retry_password(&self,account:&str)->Result<String>{
        if self.text(ACCOUNT).trim().eq_ignore_ascii_case(account)&&!self.text(PASSWORD).is_empty(){return Ok(self.text(PASSWORD));}
        crate::credentials::read(account).map_err(|_|"请在对应模块输入历史任务账号的密码，再重试。".into())
    }
    pub(super) fn retry_distribution(&mut self){
        if self.busy{return;}
        let targets=self.records[1].iter().filter(|row|row["state"]!="成功").filter_map(|row|row["computer"].as_str().map(str::to_owned)).collect::<Vec<_>>();
        if targets.is_empty(){self.set(STATUS,"没有失败或未确认的分发目标。");return;}
        let Some(request)=self.page_state.last_distribution.clone()else{self.set(STATUS,"没有可以重试的分发配置。");return;};
        self.retry_request=Some((targets,request));self.launch(Some("retry-history"));
    }
    pub(super) unsafe fn build_pages(&mut self){unsafe{
        for (id,text) in [(800,"免 Agent 文件夹分发"),(801,"源文件通过管理端内存流式转发；不会执行文件，也不会删除目标目录中的其他内容。"),(802,"源文件或文件夹"),(803,"目标电脑与凭据"),(804,"源电脑名 / IP"),(805,"源电脑读取账号"),(806,"目标路径预设"),(807,"并发电脑数"),(808,"单机文件并发"),(809,"完整性校验"),(810,"同名文件处理"),(811,"实时分发任务"),(812,"普通复制模式：文件先写入临时文件，校验完成后再替换正式文件。")]{self.add(id,"STATIC",text,SS_LEFT,236,0,1,1,1);}
        self.add(LOCAL_SOURCE,"BUTTON","管理机本地文件/文件夹",WS_TABSTOP|WS_GROUP|BS_AUTORADIOBUTTON as u32,236,0,1,1,1);
        self.add(REMOTE_SOURCE,"BUTTON","远程源电脑文件/文件夹",WS_TABSTOP|BS_AUTORADIOBUTTON as u32,236,0,1,1,1);
        SendMessageW(self.handle(LOCAL_SOURCE),BM_SETCHECK,BST_CHECKED as usize,0);
        for id in [SOURCE_COMPUTER,SOURCE_ACCOUNT,SOURCE_PASSWORD]{self.add(id,"EDIT","",WS_TABSTOP|ES_AUTOHSCROLL as u32|if id==SOURCE_PASSWORD{ES_PASSWORD as u32}else{0},236,0,1,1,1);}
        self.add(REMEMBER_SOURCE,"BUTTON","保存源凭据",WS_TABSTOP|BS_AUTOCHECKBOX as u32,236,0,1,1,1);
        for (id,items,selected) in [
            (DESTINATION_PRESET,vec!["自定义绝对路径","公共桌面","公共开始菜单","ProgramData\\Company","选定用户桌面","选定用户文档","选定用户下载目录"],0),
            (TARGET_CONCURRENCY,vec!["1","2","4","8","12","16"],2),(FILE_CONCURRENCY,vec!["1","2","4","8","16"],2),
            (VERIFICATION,vec!["快速（可执行文件 SHA-256）","完整（所有文件 SHA-256）"],0)
        ]{
            self.add(id,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWNLIST as u32,236,0,1,1,1);
            for item in items{SendMessageW(self.handle(id),CB_ADDSTRING,0,wide(item).as_ptr() as isize);}SendMessageW(self.handle(id),CB_SETCURSEL,selected,0);
        }
        self.add(FILTERS,"BUTTON","过滤规则…",WS_TABSTOP|BS_PUSHBUTTON as u32,236,0,1,1,1);
        self.add(RETRY_DISTRIBUTION,"BUTTON","重试失败电脑",WS_TABSTOP|BS_PUSHBUTTON as u32,236,0,1,1,1);
        self.add(DETAIL_TABS,"SysTabControl32","",WS_TABSTOP,236,0,1,1,3);
        for (index,label) in ["概览","已安装软件","网络","磁盘","登录会话","警告"].iter().enumerate(){
            let mut label=wide(label);let tab=TCITEMW{mask:TCIF_TEXT,pszText:label.as_mut_ptr(),..zeroed()};SendMessageW(self.handle(DETAIL_TABS),TCM_INSERTITEMW,index,(&tab as *const TCITEMW) as isize);
        }
        self.add(DETAIL_TABLE,"SysListView32","",WS_TABSTOP|LVS_REPORT|LVS_SHOWSELALWAYS,236,0,1,1,3);
        SendMessageW(self.handle(DETAIL_TABLE),LVM_SETEXTENDEDLISTVIEWSTYLE,0,(LVS_EX_FULLROWSELECT|LVS_EX_DOUBLEBUFFER) as isize);
        self.add(DETAIL_OVERVIEW,"EDIT","",WS_TABSTOP|WS_VSCROLL|ES_MULTILINE as u32|ES_AUTOVSCROLL as u32|ES_READONLY as u32,236,0,1,1,3);
        for (id,label) in [(EXPORT_JSON,"导出 JSON"),(EXPORT_CSV,"导出 CSV"),(EXPORT_EXCEL,"导出 Excel"),(EXPORT_SCOPE,"导出内容：全部"),(REPAIR_NETWORK,"连接诊断与修复")]{self.add(id,"BUTTON",label,WS_TABSTOP|BS_PUSHBUTTON as u32,236,0,1,1,3);}
        self.add(LOG_CONCURRENCY,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWNLIST as u32,236,0,1,1,9);
        for count in ["1","2","4","8"]{SendMessageW(self.handle(LOG_CONCURRENCY),CB_ADDSTRING,0,wide(count).as_ptr() as isize);}SendMessageW(self.handle(LOG_CONCURRENCY),CB_SETCURSEL,2,0);
        self.add(LOG_OPTIONS+8,"BUTTON","Security 日志（敏感）",WS_TABSTOP|BS_AUTOCHECKBOX as u32,236,0,1,1,9);
        for (id,label,page) in [(BCU_LICENSE,"查看许可",7),(CLEAR_DIAGNOSTICS,"清空结果",5),(OPEN_BUNDLE,"打开诊断包",9),(OPEN_BUNDLE_FOLDER,"打开所在目录",9)]{self.add(id,"BUTTON",label,WS_TABSTOP|BS_PUSHBUTTON as u32,236,0,1,1,page);}
        for (id,label,page) in [(813,"并发电脑数",9),(814,"单机超时（分钟）",3),(815,"采集内容（默认全部）",3),(816,"电脑资产概览",3),(817,"诊断场景（可多选）",9),(818,"诊断包内容（可单项或全部）",9),(819,"实时诊断任务",9),(820,"Windows Security 日志维护",8),(821,"清理 Security 日志并设置为按需覆盖事件（旧事件优先），保持容量不变；不保存 EVTX。",8),(822,"维护任务",8)]{
            self.add(id,"STATIC",label,SS_LEFT,236,0,1,1,page);
        }
        self.add(DISPLAY_NAME,"EDIT","",WS_TABSTOP|ES_AUTOHSCROLL as u32,236,0,1,1,0);
        for (id,text,checked) in [(MACHINE_WIDE,"所有用户安装（管理员）",true),(START_MENU,"修复开始菜单快捷方式",true),(DESKTOP_SHORTCUT,"创建桌面快捷方式",true),(ALLOW_USER_CANCEL,"允许用户取消",true),(PUBLIC_DESKTOP,"桌面快捷方式放到公共桌面（所有用户可见）",false),(ELEVATED_SHORTCUT,"安装后创建提权快捷方式（不创建普通桌面入口）",false),(ROLLBACK_ENABLED,"失败时执行回滚命令",false)]{
            self.add(id,"BUTTON",text,WS_TABSTOP|BS_AUTOCHECKBOX as u32,236,0,1,1,0);SendMessageW(self.handle(id),BM_SETCHECK,if checked{BST_CHECKED}else{BST_UNCHECKED} as usize,0);
        }
        EnableWindow(self.handle(MACHINE_WIDE),0);
        for id in [ROLLBACK_COMMAND,ROLLBACK_ARGUMENTS]{self.add(id,"EDIT","",WS_TABSTOP|ES_AUTOHSCROLL as u32,236,0,1,1,0);}
        self.add(UPLOAD_PACKAGE,"BUTTON","从管理机上传",WS_TABSTOP|BS_AUTOCHECKBOX as u32,236,0,1,1,0);SendMessageW(self.handle(UPLOAD_PACKAGE),BM_SETCHECK,BST_CHECKED as usize,0);
        for (id,text) in [(823,"软件名称（可选）"),(824,"使用上方管理员账号运行安装器；请在安装器中选择“所有用户”。默认给目标登录用户补建桌面快捷方式。"),(825,"高级选项（失败回滚）"),(826,"源文件位置"),(827,"执行参数")]{self.add(id,"STATIC",text,SS_LEFT,236,0,1,1,0);}
        for (id,text) in [(870,"LIVE REMOTE WORK QUEUE"),(871,"0  ACTIVE"),(872,"任务在切换页面后持续运行；重启后孤立的未完成任务标记为管理端已中断。")]{self.add(id,"STATIC",text,SS_LEFT,236,0,1,1,11);}
        self.add(TASK_HISTORY,"BUTTON","打开历史目录",WS_TABSTOP|BS_PUSHBUTTON as u32,236,0,1,1,11);
        for (id,text) in [(828,"统一任务中心"),(829,"跨模块查看、取消和追溯远程任务；同一电脑的软件变更自动串行。"),(830,"执行轨道")]{self.add(id,"STATIC",text,SS_LEFT,236,0,1,1,11);}
        for (id,text) in [(831,"1  目标"),(832,"2  会话"),(833,"3  安装包"),(834,"4  执行"),(835,"新建部署任务"),(836,"目标与会话"),(837,"目标交互用户"),(838,"执行前检查"),(839,"实时任务队列"),(840,"执行前会检查目标连接、管理员凭据和安装包路径。"),(841,"回滚程序"),(842,"回滚参数")]{self.add(id,"STATIC",text,SS_LEFT,236,0,1,1,0);}
        for (id,text) in [(843,"让用户一键启动所需软件"),(844,"选择目标电脑与软件，为用户创建管理员启动入口。"),(845,"连接目标电脑"),(846,"选择要授权的软件"),(847,"创建时将自动识别登录用户。"),(848,"首次创建会配置目标电脑的提权服务，并保存管理员运行凭据。"),(849,"软件候选入口（多个候选时手动选择）")]{self.add(id,"STATIC",text,SS_LEFT,236,0,1,1,10);}
        for (id,text) in [(ELEVATION_BROWSE,"浏览软件"),(ELEVATION_DISCOVER,"查找软件入口"),(ELEVATION_LIST,"刷新授权列表")]{self.add(id,"BUTTON",text,WS_TABSTOP|BS_PUSHBUTTON as u32,236,0,1,1,10);}
        for (id,text) in [(ELEVATION_ADVANCED,"高级选项和软件详情"),(ELEVATION_MANAGE,"已有授权管理")]{self.add(id,"BUTTON",text,WS_TABSTOP|BS_AUTOCHECKBOX as u32,236,0,1,1,10);}
        self.add(ELEVATION_FILTER,"EDIT","",WS_TABSTOP|ES_AUTOHSCROLL as u32,236,0,1,1,10);
        self.add(ELEVATION_CANDIDATE,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWNLIST as u32,236,0,1,1,10);
        self.add(ELEVATION_PREVIEW,"EDIT","",WS_TABSTOP|WS_VSCROLL|ES_MULTILINE as u32|ES_READONLY as u32,236,0,1,1,10);
        self.add(ELEVATION_RECEIPT,"EDIT","",WS_TABSTOP|WS_VSCROLL|ES_MULTILINE as u32|ES_AUTOVSCROLL as u32|ES_READONLY as u32,236,0,1,1,10);
    }}
    pub(super) fn distribution_options(&self)->crate::distribution::Options{
        let index=|id|unsafe{SendMessageW(self.handle(id),CB_GETCURSEL,0,0).max(0) as usize};
        let mut options=self.page_state.filters.clone();
        options.source_computer=if self.checked(REMOTE_SOURCE){self.text(SOURCE_COMPUTER).trim().into()}else{String::new()};
        options.source_account=if self.checked(REMOTE_SOURCE){self.text(SOURCE_ACCOUNT).trim().into()}else{String::new()};
        options.destination_preset=PRESETS.get(index(DESTINATION_PRESET)).unwrap_or(&"Custom").to_string();
        options.target_concurrency=[1,2,4,8,12,16].get(index(TARGET_CONCURRENCY)).copied().unwrap_or(4);
        options.file_concurrency=[1,2,4,8,16].get(index(FILE_CONCURRENCY)).copied().unwrap_or(4);
        options.full_verification=index(VERIFICATION)==1;options
    }
    pub(super) fn install_options(&self)->crate::install_options::Options{
        crate::install_options::Options{display_name:self.text(DISPLAY_NAME).trim().into(),start_menu:self.checked(START_MENU),desktop:self.checked(DESKTOP_SHORTCUT),allow_user_cancel:self.checked(ALLOW_USER_CANCEL),public_desktop:self.checked(PUBLIC_DESKTOP),elevated_shortcut:self.checked(ELEVATED_SHORTCUT),rollback:self.checked(ROLLBACK_ENABLED),rollback_command:self.text(ROLLBACK_COMMAND).trim().into(),rollback_arguments:self.text(ROLLBACK_ARGUMENTS)}
    }
    pub(super) fn source_credentials(&self,request:&Request,target:&Credentials)->Result<Option<Credentials>>{
        if request.operation!="distribute"||request.distribution.source_computer.is_empty(){return Ok(None);}
        let account=&request.distribution.source_account;
        if account.is_empty(){return Ok(Some(target.clone()));}
        let password=if self.text(SOURCE_ACCOUNT).trim().eq_ignore_ascii_case(account)&&!self.text(SOURCE_PASSWORD).is_empty(){self.text(SOURCE_PASSWORD)}else{crate::credentials::read(account).map_err(|_|"请输入远程源电脑的读取密码，或选择已经保存的源账号。")?};
        Ok(Some(Credentials{account:account.clone(),password}))
    }
    pub(super) fn edit_filters(&mut self){
        use crate::form::{self,Field};let options=&self.page_state.filters;
        let fields=vec![Field::text("仅包含文件",&options.include_files),Field::text("排除文件",&options.exclude_files),Field::text("排除目录",&options.exclude_directories),Field::text("最大文件（MiB）",&options.maximum_file_bytes.map(|n|(n/1048576).to_string()).unwrap_or_default())];
        match form::show(self.window,"过滤规则（可选）","支持 *、? 通配符，多个规则用分号分隔；最大文件留空表示不限。",fields){
            Ok(Some(values))=>{
                let maximum=if values[3].trim().is_empty(){None}else{match values[3].trim().parse::<u64>().ok().and_then(|n|n.checked_mul(1048576)){Some(n)=>Some(n),None=>{self.set(STATUS,"最大文件大小请输入有效的非负整数。");return;}}};
                let options=&mut self.page_state.filters;options.include_files=values[0].clone();options.exclude_files=values[1].clone();options.exclude_directories=values[2].clone();options.maximum_file_bytes=maximum;
            },Ok(None)=>(),Err(error)=>self.set(STATUS,&error)
        }
    }
    pub(super) unsafe fn layout_distribution(&self,rect:&RECT){unsafe{
        let px=|n:i32|(n as f64*self.scale).round() as i32;let bottom=(rect.bottom as f64/self.scale) as i32;
        let left=241;let width=((rect.right as f64/self.scale) as i32-left-36).max(720);let half=(width-18)/2;let right=left+half+18;
        for control in self.controls.iter().filter(|control|GetParent(control.handle)==self.canvas||matches!(control.id,TITLE|529)){ShowWindow(control.handle,SW_HIDE);}
        let place=|id,x,y,w,h|{canvas::move_control(self,self.handle(id),px(x),px(y),px(w),px(h));ShowWindow(self.handle(id),SW_SHOW);};
        place(529,225,10,width,20);place(TITLE,225,34,width-150,36);place(800,left+16,112,width-32,28);place(801,left+16,146,width-32,28);
        place(802,left+14,192,half-28,24);place(803,right+14,192,half-28,24);
        place(LOCAL_SOURCE,left+14,220,(half-28)/2,28);place(REMOTE_SOURCE,left+14+(half-28)/2,220,(half-28)/2,28);
        place(DISTRIBUTION_SOURCE,left+14,252,half-208,240);place(BROWSE,left+half-184,252,82,34);place(SECONDARY,left+half-94,252,82,34);
        let source_width=(half-40)*2/5;
        place(804,left+14,294,source_width,24);place(SOURCE_COMPUTER,left+14,320,source_width,34);
        place(805,left+source_width+26,294,half-source_width-40,24);place(SOURCE_ACCOUNT,left+source_width+26,320,half-source_width-40,34);
        place(SOURCE_PASSWORD,left+14,364,(half-40)/2,34);place(REMEMBER_SOURCE,left+26+(half-40)/2,366,(half-40)/2,28);
        let targets=(half-42)*3/5;let account=right+targets+28;let credentials_width=half-targets-42;
        place(505,right+14,224,targets,24);place(TARGET,right+14,252,targets,96);
        place(506,account,224,credentials_width,24);place(ACCOUNT,account,252,credentials_width,200);
        place(507,account,294,credentials_width,24);place(PASSWORD,account,320,credentials_width,34);
        place(REMEMBER_CREDENTIALS,right+14,366,half-28,28);
        let preset=160;let verification=176;let file=85;let concurrency=80;let destination=width-preset-verification-file-concurrency-48;
        let destination_x=left+preset+12;let concurrency_x=destination_x+destination+12;let file_x=concurrency_x+concurrency+12;let verify_x=file_x+file+12;
        for (label,control,x,w) in [(806,DESTINATION_PRESET,left,preset),(515,DESTINATION,destination_x,destination),(807,TARGET_CONCURRENCY,concurrency_x,concurrency),(808,FILE_CONCURRENCY,file_x,file),(809,VERIFICATION,verify_x,verification)]{
            place(label,x,414,w,24);place(control,x,442,w,if matches!(control,DESTINATION_PRESET|TARGET_CONCURRENCY|FILE_CONCURRENCY|VERIFICATION){200}else{38});
        }
        place(810,left,492,240,24);place(OVERWRITE,left,520,240,180);place(812,left+254,496,width-390,54);place(FILTERS,left+width-124,518,124,34);
        let selected=SendMessageW(self.handle(DESTINATION_PRESET),CB_GETCURSEL,0,0);
        if selected>=4{place(SESSION,left+254,554,width-410,180);place(SESSIONS,left+width-144,554,144,30);}
        place(811,left,596,width,24);place(RESULT_TABLE,left,628,width,(bottom-698).max(110));
        place(STATUS,left,bottom-44,width-490,30);place(RETRY_DISTRIBUTION,left+width-480,bottom-48,145,36);place(CANCEL,left+width-322,bottom-48,140,36);place(RUN,left+width-168,bottom-48,168,36);
        let remote=self.checked(REMOTE_SOURCE);for id in [SOURCE_COMPUTER,SOURCE_ACCOUNT,SOURCE_PASSWORD,REMEMBER_SOURCE]{EnableWindow(self.handle(id),(remote&&!self.busy) as i32);}
        EnableWindow(self.handle(DESTINATION),(selected==0&&!self.busy) as i32);EnableWindow(self.handle(RETRY_DISTRIBUTION),(!self.busy) as i32);
    }}
    pub(super) unsafe fn layout_inventory(&self,rect:&RECT){unsafe{
        let px=|n:i32|(n as f64*self.scale).round() as i32;let bottom=(rect.bottom as f64/self.scale) as i32;
        let left=241;let width=((rect.right as f64/self.scale) as i32-left-36).max(720);let first=(width-36)*125/380;let second=(width-36)*110/380;let x2=left+first+18;let x3=x2+second+18;let third=left+width-x3;
        for control in self.controls.iter().filter(|control|GetParent(control.handle)==self.canvas||matches!(control.id,TITLE|529)){ShowWindow(control.handle,SW_HIDE);}
        let place=|id,x,y,w,h|{canvas::move_control(self,self.handle(id),px(x),px(y),px(w),px(h));ShowWindow(self.handle(id),SW_SHOW);};
        place(529,225,10,width,20);place(TITLE,225,34,width-150,36);
        place(505,left,120,first,24);place(TARGET,left,148,first,106);
        place(506,x2,120,second,24);place(ACCOUNT,x2,148,second,200);place(507,x2,196,second,24);place(PASSWORD,x2,224,second,38);place(REMEMBER_CREDENTIALS,x2,270,second,42);
        place(521,x3,120,(third-12)/2,24);place(INVENTORY_CONCURRENCY,x3,148,(third-12)/2,180);place(814,x3+(third+12)/2,120,(third-12)/2,24);place(TIMEOUT,x3+(third+12)/2,148,(third-12)/2,38);
        place(815,x3,200,third,24);for index in 0..8{place(INVENTORY_OPTIONS+index,x3+(index%4)*(third/4),230+(index/4)*34,third/4,28);}
        place(816,left,340,width-280,24);place(STATUS,left+width-280,340,280,24);
        splitter::layout(self, left, width, bottom);
        for (id,x,w) in [(EXPORT_SCOPE,left,150),(EXPORT_JSON,left+160,78),(EXPORT_CSV,left+248,78),(EXPORT_EXCEL,left+336,86),(REPAIR_NETWORK,left+434,146),(CANCEL,left+width-286,125),(RUN,left+width-148,148)]{place(id,x,bottom-48,w,36);}
    }}
    pub(super) unsafe fn layout_security(&self,rect:&RECT){unsafe{
        let px=|n:i32|(n as f64*self.scale).round() as i32;let bottom=(rect.bottom as f64/self.scale) as i32;
        let left=241;let width=((rect.right as f64/self.scale) as i32-left-36).max(600);let half=(width-20)/2;let right=left+half+20;
        for control in self.controls.iter().filter(|control|GetParent(control.handle)==self.canvas||matches!(control.id,TITLE|529)){ShowWindow(control.handle,SW_HIDE);}
        let place=|id,x,y,w,h|{canvas::move_control(self,self.handle(id),px(x),px(y),px(w),px(h));ShowWindow(self.handle(id),SW_SHOW);};
        place(529,225,10,width,20);place(TITLE,225,34,width-150,36);place(820,left,112,width,28);place(821,left,146,width,44);
        place(505,left,200,half,24);place(TARGET,left,228,half,88);place(506,right,200,half,24);place(ACCOUNT,right,228,half,200);
        place(507,right,272,half,24);place(PASSWORD,right,300,half,38);place(REMEMBER_CREDENTIALS,right,348,half,34);
        place(822,left,410,width,28);place(RESULT_TABLE,left,450,width,(bottom-520).max(160));
        place(STATUS,left,bottom-44,width-340,30);place(CANCEL,left+width-310,bottom-48,125,36);place(RUN,left+width-172,bottom-48,172,36);
    }}
    pub(super) unsafe fn layout_logs(&self,rect:&RECT){unsafe{
        let px=|n:i32|(n as f64*self.scale).round() as i32;let bottom=(rect.bottom as f64/self.scale) as i32;
        let left=241;let width=((rect.right as f64/self.scale) as i32-left-36).max(720);let first=(width-36)*110/380;let second=(width-36)*115/380;let x2=left+first+18;let x3=x2+second+18;let third=left+width-x3;
        for control in self.controls.iter().filter(|control|GetParent(control.handle)==self.canvas||matches!(control.id,TITLE|529)){ShowWindow(control.handle,SW_HIDE);}
        let place=|id,x,y,w,h|{canvas::move_control(self,self.handle(id),px(x),px(y),px(w),px(h));ShowWindow(self.handle(id),SW_SHOW);};
        place(529,225,10,width,20);place(TITLE,225,34,width-150,36);place(505,left,120,first,24);place(TARGET,left,148,first,104);
        place(506,x2,120,second,24);place(ACCOUNT,x2,148,second,200);place(507,x2,196,second,24);place(PASSWORD,x2,224,second,38);place(REMEMBER_CREDENTIALS,x2,270,second,42);
        let cell=(third-20)/3;for (id,label,index) in [(LOG_RANGE,523,0),(LOG_CONCURRENCY,813,1),(TIMEOUT,513,2)]{place(label,x3+index*(cell+10),120,cell,34);place(id,x3+index*(cell+10),160,cell,if id==TIMEOUT{38}else{200});}
        place(817,x3,212,third,24);for index in 0..4{place(LOG_OPTIONS+index,x3+(index%2)*(third/2),242+(index/2)*30,third/2,28);}
        place(818,x3,308,third,24);for index in 0..5{place(LOG_OPTIONS+4+index,x3+(index%2)*(third/2),338+(index/2)*30,third/2,28);}
        place(819,left,446,width-300,24);place(STATUS,left+width-300,446,300,24);
        splitter::layout(self, left, width, bottom);
        place(CANCEL,left+width-310,bottom-48,125,36);place(RUN,left+width-172,bottom-48,172,36);
    }}
    pub(super) unsafe fn layout_bcu(&self,rect:&RECT){unsafe{
        let px=|n:i32|(n as f64*self.scale).round() as i32;let bottom=(rect.bottom as f64/self.scale) as i32;
        let left=241;let width=((rect.right as f64/self.scale) as i32-left-36).max(720);let cell=(width-126)/3;
        for control in self.controls.iter().filter(|control|GetParent(control.handle)==self.canvas||matches!(control.id,TITLE|529)){ShowWindow(control.handle,SW_HIDE);}
        let place=|id,x,y,w,h|{canvas::move_control(self,self.handle(id),px(x),px(y),px(w),px(h));ShowWindow(self.handle(id),SW_SHOW);};
        place(529,225,10,width,20);place(TITLE,225,34,width-150,36);
        for (label,control,index) in [(505,TARGET,0),(506,ACCOUNT,1),(507,PASSWORD,2)]{let x=left+index*(cell+12);place(label,x,120,cell,24);place(control,x,150,cell,if control==ACCOUNT{200}else{38});}
        place(513,left+width-90,120,90,28);place(TIMEOUT,left+width-90,150,90,38);place(REMEMBER_CREDENTIALS,left,204,width,28);
        place(512,left,252,120,24);place(SESSION,left+126,246,width-290,200);place(SESSIONS,left+width-150,246,150,36);
        place(524,left,304,width,24);place(SOFTWARE_SEARCH,left,334,width-190,38);place(RUN,left+width-176,334,176,38);
        splitter::layout(self, left, width, bottom);
        place(BCU_LICENSE,left,bottom-48,110,36);place(STATUS,left+124,bottom-42,width-462,30);place(CANCEL,left+width-310,bottom-48,125,36);place(SECONDARY,left+width-172,bottom-48,172,36);
    }}
    pub(super) unsafe fn layout_diagnostics(&self,rect:&RECT){unsafe{
        let px=|n:i32|(n as f64*self.scale).round() as i32;let bottom=(rect.bottom as f64/self.scale) as i32;
        let left=241;let width=((rect.right as f64/self.scale) as i32-left-36).max(720);let cell=(width-278)/3;
        for control in self.controls.iter().filter(|control|GetParent(control.handle)==self.canvas||matches!(control.id,TITLE|529)){ShowWindow(control.handle,SW_HIDE);}
        let place=|id,x,y,w,h|{canvas::move_control(self,self.handle(id),px(x),px(y),px(w),px(h));ShowWindow(self.handle(id),SW_SHOW);};
        place(529,225,10,width,20);place(TITLE,225,34,width-150,36);
        for (label,control,index) in [(505,TARGET,0),(506,ACCOUNT,1),(507,PASSWORD,2)]{let x=left+index*(cell+12);place(label,x,120,cell,24);place(control,x,150,cell,if control==ACCOUNT{200}else if control==TARGET{76}else{38});}
        place(513,left+width-242,120,100,28);place(TIMEOUT,left+width-242,150,100,38);place(RUN,left+width-128,150,128,38);place(SECONDARY,left+width-128,198,128,38);
        place(HINT,left,262,width,36);place(STATUS,left,310,width,26);place(RESULT_TABLE,left,350,width,(bottom-420).max(180));
        place(CLEAR_DIAGNOSTICS,left+width-140,bottom-48,140,36);place(CANCEL,left+width-278,bottom-48,125,36);
    }}
    pub(super) unsafe fn layout_install(&self,rect:&RECT){unsafe{
        let px=|n:i32|(n as f64*self.scale).round() as i32;let bottom=(rect.bottom as f64/self.scale) as i32;
        let left=241;let width=((rect.right as f64/self.scale) as i32-left-36).max(720);let form=width-324;let right=left+form+32;let half=(form-14)/2;
        for control in self.controls.iter().filter(|control|GetParent(control.handle)==self.canvas||matches!(control.id,TITLE|529)){ShowWindow(control.handle,SW_HIDE);}
        let place=|id,x,y,w,h|{canvas::move_control(self,self.handle(id),px(x),px(y),px(w),px(h));ShowWindow(self.handle(id),SW_SHOW);};
        place(529,225,10,width,20);place(TITLE,225,34,width-150,36);
        for index in 0..4{place(831+index,left+index*(width/4),116,width/4,42);}
        place(835,left,190,form,28);place(505,left,236,half,24);place(TARGET,left,264,half,56);
        place(506,left+half+14,236,half,24);place(ACCOUNT,left+half+14,264,half-118,200);place(SAVE_ACCOUNT,left+form-108,264,48,38);place(DELETE_ACCOUNT,left+form-54,264,54,38);
        place(507,left,334,form-184,24);place(PASSWORD,left,362,form-184,38);place(513,left+form-170,334,170,24);place(TIMEOUT,left+form-170,362,170,38);
        place(REMEMBER_CREDENTIALS,left,410,form,28);place(826,left,452,110,24);place(UPLOAD_PACKAGE,left+116,450,180,28);place(EXISTING,left+300,450,form-300,28);
        place(520,left,496,105,24);place(EXECUTION_MODE,left+112,490,260,180);place(510,left,540,form,24);place(SOURCE,left,568,form-104,240);place(BROWSE,left+form-94,568,94,40);
        place(823,left,622,half,24);place(DISPLAY_NAME,left,650,half,38);place(511,left+half+14,622,half,24);place(ARGUMENTS,left+half+14,650,half,38);
        place(MACHINE_WIDE,left,700,half,28);place(START_MENU,left+half+14,700,half,28);place(DESKTOP_SHORTCUT,left,736,half,28);place(ALLOW_USER_CANCEL,left+half+14,736,half,28);
        place(PUBLIC_DESKTOP,left,774,form,28);place(ELEVATED_SHORTCUT,left,812,form,28);place(824,left,852,form,58);
        place(825,left,926,form,26);place(ROLLBACK_ENABLED,left,960,form,28);place(841,left,1000,half,24);place(ROLLBACK_COMMAND,left,1028,half,38);place(842,left+half+14,1000,half,24);place(ROLLBACK_ARGUMENTS,left+half+14,1028,half,38);
        place(836,right+14,190,282,28);place(837,right+14,244,282,24);place(SESSION,right+14,276,282,200);place(SESSIONS,right+14,324,282,38);place(HINT,right+14,382,282,104);
        place(838,right+14,506,282,26);place(840,right+14,544,282,86);place(RUN,right+14,656,282,42);
        place(839,left,1100,width,28);place(RESULT_TABLE,left,1140,width,(bottom-1220).max(180));place(STATUS,left,bottom-44,width-170,30);place(CANCEL,left+width-150,bottom-48,150,36);
        let install=!self.direct_run_selected();for id in [DISPLAY_NAME,MACHINE_WIDE,START_MENU,DESKTOP_SHORTCUT,ALLOW_USER_CANCEL,PUBLIC_DESKTOP,ELEVATED_SHORTCUT,ROLLBACK_ENABLED,ROLLBACK_COMMAND,ROLLBACK_ARGUMENTS]{EnableWindow(self.handle(id),(install&&id!=MACHINE_WIDE) as i32);}
        for id in [ROLLBACK_COMMAND,ROLLBACK_ARGUMENTS]{EnableWindow(self.handle(id),(install&&self.checked(ROLLBACK_ENABLED)) as i32);}
        for id in [DESKTOP_SHORTCUT,PUBLIC_DESKTOP]{EnableWindow(self.handle(id),(install&&!self.checked(ELEVATED_SHORTCUT)) as i32);}
    }}
    pub(super) unsafe fn layout_tasks(&self,rect:&RECT){unsafe{
        let px=|n:i32|(n as f64*self.scale).round() as i32;let bottom=(rect.bottom as f64/self.scale) as i32;let left=241;let width=((rect.right as f64/self.scale) as i32-left-36).max(600);
        for control in self.controls.iter().filter(|control|GetParent(control.handle)==self.canvas||matches!(control.id,TITLE|529)){ShowWindow(control.handle,SW_HIDE);}
        let place=|id,x,y,w,h|{canvas::move_control(self,self.handle(id),px(x),px(y),px(w),px(h));ShowWindow(self.handle(id),SW_SHOW);};
        place(529,225,10,width,20);place(TITLE,225,34,width-150,36);place(870,left+20,112,width-200,22);place(828,left+20,136,width-200,34);place(829,left+20,178,width-200,38);place(871,left+width-150,140,140,42);
        place(830,left+12,242,width-474,24);place(872,left+12,272,width-320,20);place(CANCEL,left+width-292,240,140,36);place(TASK_HISTORY,left+width-140,240,140,36);
        place(RESULT_TABLE,left,304,width,(bottom-364).max(200));place(STATUS,left,bottom-44,width-340,30);place(RUN,left+width-320,bottom-48,140,36);place(SECONDARY,left+width-168,bottom-48,168,36);
        self.set(RUN,"刷新任务");self.set(CANCEL,"取消所选任务");EnableWindow(self.handle(CANCEL),1);
    }}
    pub(super) unsafe fn layout_elevation(&self,rect:&RECT){unsafe{
        let px=|n:i32|(n as f64*self.scale).round() as i32;let left=237;let width=((rect.right as f64/self.scale) as i32-left-32).min(1080).max(680);let cell=(width-32)/3;
        for control in self.controls.iter().filter(|control|GetParent(control.handle)==self.canvas||matches!(control.id,TITLE|529)){ShowWindow(control.handle,SW_HIDE);}
        let place=|id,x,y,w,h|{canvas::move_control(self,self.handle(id),px(x),px(y),px(w),px(h));ShowWindow(self.handle(id),SW_SHOW);};
        place(529,225,10,width,20);place(TITLE,225,34,width-150,36);place(843,left,116,width,34);place(844,left,158,width,28);place(845,left+22,214,width-44,24);
        for (label,control,index) in [(505,TARGET,0),(506,ACCOUNT,1),(507,PASSWORD,2)]{let x=left+22+index*(cell+16);place(label,x,252,cell-16,24);place(control,x,284,cell-16,if control==ACCOUNT{200}else{42});}
        place(REMEMBER_CREDENTIALS,left+22,344,width-44,28);place(846,left+22,414,width-44,24);place(ELEVATION_PATH,left+22,452,width-158,42);place(ELEVATION_BROWSE,left+width-124,452,102,42);
        place(512,left+34,526,72,24);place(SESSION,left+112,518,width-146,200);place(847,left+22,568,width-44,28);place(RUN,left+22,618,214,44);place(848,left+22,682,width-44,38);place(STATUS,left,742,width,36);
        let receipt_height = self.elevation_receipt_height();
        if receipt_height > 0 { place(ELEVATION_RECEIPT, left, 780, width, receipt_height - 10); }
        place(ELEVATION_ADVANCED,left,802 + receipt_height,width,28);let mut y=846 + receipt_height;
        if self.checked(ELEVATION_ADVANCED){
            for (id,x,w) in [(SESSIONS,left,150),(SAVE_ACCOUNT,left+164,130),(DELETE_ACCOUNT,left+308,130)]{place(id,x,y,w,36);}y+=54;
            place(ELEVATION_UPDATE,left,y,width,28);y+=44;place(ELEVATION_DISCOVER,left,y,150,36);place(ELEVATION_FILTER,left+164,y,250,36);place(SECONDARY,left+430,y,150,36);y+=54;
            place(849,left,y,width,24);y+=32;place(ELEVATION_CANDIDATE,left,y,width,200);y+=50;place(ELEVATION_PREVIEW,left,y,width,120);y+=144;
        }
        place(ELEVATION_MANAGE,left,y,width,28);y+=44;
        if self.checked(ELEVATION_MANAGE){place(ELEVATION_LIST,left,y,150,36);place(THIRD,left+164,y,150,36);y+=54;place(RESULT_TABLE,left,y,width,180);}
    }}
    pub(super) fn elevation_receipt_height(&self) -> i32 {
        if self.text(ELEVATION_RECEIPT).is_empty() { 0 } else { 112 }
    }
    pub(super) fn export_scope(&mut self){
        let names=["概览、处理器和内存","已安装软件","网络","磁盘","登录会话","补丁","服务","警告"];
        let fields=names.iter().zip(self.page_state.export_scope).map(|(name,value)|crate::form::Field::check(name,value)).collect();
        match crate::form::show(self.window,"导出内容","勾选需要导出的分类。此设置独立于采集范围。",fields){
            Ok(Some(values))=>{for (index,value) in values.iter().enumerate(){self.page_state.export_scope[index]=value=="1";}self.set(EXPORT_SCOPE,if self.page_state.export_scope.iter().all(|value|*value){"导出内容：全部"}else{"导出内容：已选择"});},Ok(None)=>(),Err(error)=>self.set(STATUS,&error)
        }
    }
    pub(super) fn show_inventory_details(&self){unsafe{
        let Some(row)=self.selected_record()else{return;};let data=&row["data"];let tab=SendMessageW(self.handle(DETAIL_TABS),TCM_GETCURSEL,0,0);
        if tab==0{
            let mut overview=data.clone();if let Some(object)=overview.as_object_mut(){for key in ["installedSoftware","networkAdapters","logicalDisks","physicalDisks","sessions","hotfixes","services"]{object.remove(key);}}
            self.set(DETAIL_OVERVIEW,&serde_json::to_string_pretty(&overview).unwrap_or_default());return;
        }
        let (category,columns):(&str,&[(&str,&str,i32)])=match tab{
            1=>("installedSoftware",&[("名称","displayName",230),("版本","version",120),("发布者","publisher",180),("范围","scope",150),("架构","architecture",80),("安装位置","installLocation",300)]),
            2=>("networkAdapters",&[("名称","Description",180),("MAC","MACAddress",140),("IP 地址","IPAddress",230),("网关","DefaultIPGateway",160),("DNS","DNSServerSearchOrder",160)]),
            3=>("logicalDisks",&[("盘符","DeviceID",80),("卷标","VolumeName",160),("文件系统","FileSystem",110),("容量（字节）","Size",140),("可用（字节）","FreeSpace",140)]),
            4=>("sessions",&[("Session","sessionId",80),("账号","account",220),("状态","state",100),("来源","station",120),("客户端","client",150)]),
            _=>("warnings",&[("警告","warning",800)])
        };
        let list=self.handle(DETAIL_TABLE);SendMessageW(list,LVM_DELETEALLITEMS,0,0);while SendMessageW(list,LVM_DELETECOLUMN,0,0)!=0{}
        for (index,(title,_,width)) in columns.iter().enumerate(){let mut title=wide(title);let column=LVCOLUMNW{mask:LVCF_TEXT|LVCF_WIDTH,cx:(*width as f64*self.scale) as i32,pszText:title.as_mut_ptr(),..zeroed()};SendMessageW(list,LVM_INSERTCOLUMNW,index,(&column as *const LVCOLUMNW) as isize);}
        for (index,item) in data[category].as_array().into_iter().flatten().enumerate(){for (column,(_,field,_)) in columns.iter().enumerate(){
            let value=if tab==5{item}else{&item[*field]};let text=match value{serde_json::Value::Null=>String::new(),serde_json::Value::String(text)=>text.clone(),_=>value.to_string()};
            let mut text=wide(&text);let item=LVITEMW{mask:LVIF_TEXT,iItem:index as i32,iSubItem:column as i32,pszText:text.as_mut_ptr(),..zeroed()};SendMessageW(list,if column==0{LVM_INSERTITEMW}else{LVM_SETITEMW},0,(&item as *const LVITEMW) as isize);
        }}
    }}
}
