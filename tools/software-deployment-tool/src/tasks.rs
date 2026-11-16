use crate::{model::{Request, Result}, win};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::{HashSet,HashMap}, path::PathBuf, sync::{Arc,Condvar, Mutex, OnceLock, atomic::{AtomicBool, Ordering}}, time::{Duration, SystemTime, UNIX_EPOCH}};

struct LiveTask{batch:String,cancel:Arc<AtomicBool>}
static LIVE:OnceLock<Mutex<HashMap<String,LiveTask>>>=OnceLock::new();
pub struct Cancellation{pub flag:Arc<AtomicBool>,id:String}
impl Cancellation{
    pub fn register(id:&str,batch:&str,batch_cancel:&AtomicBool)->Result<Self>{
        let mut live=LIVE.get_or_init(Default::default).lock().map_err(|_|"任务取消状态异常。")?;
        let flag=Arc::new(AtomicBool::new(batch_cancel.load(Ordering::Relaxed)));
        live.insert(id.into(),LiveTask{batch:batch.into(),cancel:flag.clone()});
        Ok(Self{flag,id:id.into()})
    }
}
impl Drop for Cancellation{fn drop(&mut self){if let Some(live)=LIVE.get(){if let Ok(mut tasks)=live.lock(){tasks.remove(&self.id);}}}}
pub fn cancel_task(id:&str)->Result<()> {
    let tasks=LIVE.get_or_init(Default::default).lock().map_err(|_|"任务取消状态异常。")?;
    let task=tasks.get(id).ok_or("任务已经结束，或由另一个管理端运行。")?;
    task.cancel.store(true,Ordering::Relaxed);Ok(())
}
pub fn cancel_batch(batch:&str,flag:&AtomicBool){
    flag.store(true,Ordering::Relaxed);
    if let Some(live)=LIVE.get(){if let Ok(tasks)=live.lock(){for task in tasks.values().filter(|task|task.batch==batch){task.cancel.store(true,Ordering::Relaxed);}}}
}

pub fn timestamp() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

pub fn data_directory() -> Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or("无法找到本地应用数据目录。")?;
    let directory = PathBuf::from(base).join("CorpInstallerRust");
    win::io(std::fs::create_dir_all(&directory))?;
    Ok(directory)
}

fn database() -> Result<Connection> {
    database_at(&data_directory()?.join("tasks.sqlite"))
}
fn database_at(path:&std::path::Path)->Result<Connection>{
    let mut connection = Connection::open(path).map_err(|e|e.to_string())?;
    connection.busy_timeout(Duration::from_secs(10)).map_err(|e|e.to_string())?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS tasks (
           id TEXT PRIMARY KEY, batch TEXT NOT NULL, target TEXT NOT NULL,
           operation TEXT NOT NULL, account TEXT NOT NULL, state TEXT NOT NULL,
           detail TEXT NOT NULL DEFAULT '', progress INTEGER NOT NULL DEFAULT 0,
           created INTEGER NOT NULL, updated INTEGER NOT NULL,
           request TEXT NOT NULL, result TEXT);
         CREATE INDEX IF NOT EXISTS tasks_updated ON tasks(updated DESC);"
    ).map_err(|e|e.to_string())?;
    let transaction=connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
    let columns=transaction.prepare("PRAGMA table_info(tasks)").map_err(|e|e.to_string())?
        .query_map([],|row|row.get::<_,String>(1)).map_err(|e|e.to_string())?
        .collect::<std::result::Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    for (column,definition) in [("owner_pid","INTEGER NOT NULL DEFAULT 0"),("owner_created","TEXT NOT NULL DEFAULT ''")]{
        if !columns.iter().any(|name|name==column){transaction.execute_batch(&format!("ALTER TABLE tasks ADD COLUMN {column} {definition}")).map_err(|e|e.to_string())?;}
    }
    transaction.commit().map_err(|e|e.to_string())?;
    Ok(connection)
}

fn process_created(pid:u32)->Option<String>{
    use windows_sys::Win32::{Foundation::*,System::Threading::*};
    unsafe{
        let process=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,0,pid);
        if process.is_null(){return None;}
        let process=win::Handle(process);let mut created=FILETIME::default();let mut exited=FILETIME::default();
        let mut kernel=FILETIME::default();let mut user=FILETIME::default();let mut exit=0;
        if GetProcessTimes(process.0,&mut created,&mut exited,&mut kernel,&mut user)==0||GetExitCodeProcess(process.0,&mut exit)==0||exit!=STILL_ACTIVE as u32{return None;}
        Some(format!("{}:{}",created.dwHighDateTime,created.dwLowDateTime))
    }
}

pub fn enqueue(batch: &str, target: &str, request: &Request) -> Result<()> {
    let now = timestamp();
    database()?.execute(
        "INSERT INTO tasks(id,batch,target,operation,account,state,created,updated,request,owner_pid,owner_created)
         VALUES(?1,?2,?3,?4,?5,'Queued',?6,?6,?7,?8,?9)",
        params![request.id,batch,target,request.operation,request.account,now,
            serde_json::to_string(request).map_err(|e|e.to_string())?,std::process::id(),process_created(std::process::id()).unwrap_or_default()]
    ).map_err(|e|e.to_string())?;
    Ok(())
}

pub fn update(id: &str, state: &str, detail: &str, result: Option<&Value>) -> Result<()> {
    database()?.execute(
        "UPDATE tasks SET state=?2,detail=?3,updated=?4,result=COALESCE(?5,result),progress=CASE WHEN ?2='Succeeded' THEN 100 ELSE progress END WHERE id=?1",
        params![id,state,detail,timestamp(),result.map(Value::to_string)]
    ).map_err(|e|e.to_string())?;
    Ok(())
}
pub fn progress(id:&str,progress:u32,detail:&str)->Result<()> {
    database()?.execute("UPDATE tasks SET progress=?2,detail=?3,updated=?4 WHERE id=?1 AND state='Running'",params![id,progress.min(100),detail,timestamp()]).map_err(|e|e.to_string())?;Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub target: String,
    pub operation: String,
    pub account: String,
    pub state: String,
    pub detail: String,
    pub created: i64,
    pub updated: i64,
    pub progress:u32,
    pub request: Request,
    pub result: Option<Value>,
}

pub fn list() -> Result<Vec<Entry>> {
    let connection = database()?;
    let mut statement = connection.prepare(
        "SELECT id,target,operation,account,state,detail,created,updated,request,NULL,progress
         FROM tasks ORDER BY updated DESC LIMIT 2000"
    ).map_err(|e|e.to_string())?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,
            row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,String>(5)?,
            row.get::<_,i64>(6)?,row.get::<_,i64>(7)?,row.get::<_,String>(8)?,row.get::<_,Option<String>>(9)?,row.get::<_,u32>(10)?))
    }).map_err(|e|e.to_string())?;
    rows.map(|row| {
        let (id,target,operation,account,state,detail,created,updated,request,result,progress) = row.map_err(|e|e.to_string())?;
        Ok(Entry {id,target,operation,account,state,detail,created,updated,progress,
            request:serde_json::from_str(&request).map_err(|e|e.to_string())?,
            result:result.map(|s|serde_json::from_str(&s)).transpose().map_err(|e|e.to_string())?})
    }).collect()
}
pub fn result(id:&str)->Result<Option<Value>>{
    let text=database()?.query_row("SELECT result FROM tasks WHERE id=?1",[id],|row|row.get::<_,Option<String>>(0)).map_err(|error|error.to_string())?;
    text.map(|text|serde_json::from_str(&text).map_err(|error|error.to_string())).transpose()
}

pub fn recover_interrupted() -> Result<()> {
    let connection=database()?;
    recover(&connection)
}
fn recover(connection:&Connection)->Result<()>{
    let active=connection.prepare("SELECT id,owner_pid,owner_created FROM tasks WHERE state IN ('Queued','Running','Cancelling')").map_err(|e|e.to_string())?
        .query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,u32>(1)?,row.get::<_,String>(2)?))).map_err(|e|e.to_string())?
        .collect::<std::result::Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    for (id,pid,created) in active{
        if !created.is_empty()&&process_created(pid).as_deref()==Some(created.as_str()){continue;}
        connection.execute(
        "UPDATE tasks SET state='Unconfirmed',detail='上次管理端退出前未保存最终结果，请核对目标机。',updated=?1
         WHERE id=?2 AND state IN ('Queued','Running','Cancelling')", params![timestamp(),id]
    ).map_err(|e|e.to_string())?;
    }
    Ok(())
}

struct TargetLocks { targets: Mutex<HashSet<String>>, wake: Condvar }
static LOCKS: OnceLock<TargetLocks> = OnceLock::new();
pub struct TargetLease(String);
impl TargetLease {
    pub fn acquire(target: &str, cancelled: &AtomicBool) -> Result<Self> {
        let locks = LOCKS.get_or_init(||TargetLocks {targets:Mutex::new(HashSet::new()),wake:Condvar::new()});
        let target = target.to_ascii_lowercase();
        let mut targets = locks.targets.lock().map_err(|_|"目标锁异常。")?;
        loop {
            if cancelled.load(Ordering::Relaxed) { return Err("排队任务已取消。".into()); }
            if targets.insert(target.clone()) { return Ok(Self(target)); }
            targets = locks.wake.wait_timeout(targets,Duration::from_millis(200)).map_err(|_|"目标锁异常。")?.0;
        }
    }
}
impl Drop for TargetLease {
    fn drop(&mut self) {
        if let Some(locks) = LOCKS.get() {
            if let Ok(mut targets) = locks.targets.lock() {targets.remove(&self.0);}
            locks.wake.notify_all();
        }
    }
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn individual_and_batch_cancellation_are_isolated(){
        let batch=crate::model::new_id();let flag=AtomicBool::new(false);
        let first=Cancellation::register(&crate::model::new_id(),&batch,&flag).unwrap();
        let second=Cancellation::register(&crate::model::new_id(),&batch,&flag).unwrap();
        cancel_task(&first.id).unwrap();assert!(first.flag.load(Ordering::Relaxed));assert!(!second.flag.load(Ordering::Relaxed));
        cancel_batch(&batch,&flag);assert!(second.flag.load(Ordering::Relaxed));
        let queued=Cancellation::register(&crate::model::new_id(),&batch,&flag).unwrap();assert!(queued.flag.load(Ordering::Relaxed));
        let id=queued.id.clone();drop(queued);assert!(cancel_task(&id).is_err());
    }
    #[test]fn recovery_keeps_other_live_managers_tasks(){
        let path=std::env::temp_dir().join(format!("corp-rust-task-test-{}.sqlite",crate::model::new_id()));
        {
            let connection=database_at(&path).unwrap();
            let request=serde_json::to_string(&Request::default()).unwrap();
            for (id,pid,created) in [("live",std::process::id(),process_created(std::process::id()).unwrap()),("dead",0,String::new())]{
                connection.execute("INSERT INTO tasks(id,batch,target,operation,account,state,created,updated,request,owner_pid,owner_created) VALUES(?1,'test','fixture','sessions','test','Running',0,0,?2,?3,?4)",params![id,request,pid,created]).unwrap();
            }
            recover(&connection).unwrap();
            let state=|id:&str|connection.query_row("SELECT state FROM tasks WHERE id=?1",[id],|row|row.get::<_,String>(0)).unwrap();
            assert_eq!(state("live"),"Running");assert_eq!(state("dead"),"Unconfirmed");
        }
        std::fs::remove_file(path).unwrap();
    }
}
