use crate::{model::Result,win::{self,Handle}};
use std::{io::{self,Read,Write},mem::zeroed,ptr::{null,null_mut}};
use windows_sys::Win32::{Foundation::*,Storage::FileSystem::{ReadFile,WriteFile},System::{IO::*,Threading::*}};

pub struct Stream{pub handle:Handle}
impl Stream{
    fn transfer(&self,buffer:*mut u8,length:usize,write:bool)->io::Result<usize>{
        unsafe{
            let event=Handle(CreateEventW(null(),1,0,null()));
            if event.0.is_null(){return Err(io::Error::last_os_error());}
            let mut overlapped:OVERLAPPED=zeroed();overlapped.hEvent=event.0;let mut count=0;
            let success=if write{WriteFile(self.handle.0,buffer,length.min(u32::MAX as usize) as u32,null_mut(),&mut overlapped)}
                else{ReadFile(self.handle.0,buffer,length.min(u32::MAX as usize) as u32,null_mut(),&mut overlapped)};
            if success==0&&GetLastError()!=ERROR_IO_PENDING{return Err(io::Error::last_os_error());}
            if WaitForSingleObject(event.0,30000)!=WAIT_OBJECT_0{
                CancelIoEx(self.handle.0,&overlapped);WaitForSingleObject(event.0,INFINITE);
                return Err(io::Error::new(io::ErrorKind::TimedOut,"后台管道请求超时"));
            }
            if GetOverlappedResult(self.handle.0,&overlapped,&mut count,0)==0{return Err(io::Error::last_os_error());}
            Ok(count as usize)
        }
    }
    pub fn read_frame(&mut self)->Result<Vec<u8>>{
        let mut header=[0;4];win::io(self.read_exact(&mut header))?;
        let length=u32::from_le_bytes(header) as usize;
        if length==0||length>super::MAX_FRAME{return Err("PROTOCOL_TOO_LARGE".into());}
        let mut buffer=vec![0;length];win::io(self.read_exact(&mut buffer))?;Ok(buffer)
    }
    pub fn write_frame(&mut self,data:&[u8])->Result<()>{
        if data.is_empty()||data.len()>super::MAX_FRAME{return Err("PROTOCOL_TOO_LARGE".into());}
        win::io(self.write_all(&(data.len() as u32).to_le_bytes()))?;win::io(self.write_all(data))
    }
}
impl Read for Stream{fn read(&mut self,buffer:&mut [u8])->io::Result<usize>{self.transfer(buffer.as_mut_ptr(),buffer.len(),false)}}
impl Write for Stream{
    fn write(&mut self,buffer:&[u8])->io::Result<usize>{self.transfer(buffer.as_ptr().cast_mut(),buffer.len(),true)}
    fn flush(&mut self)->io::Result<()>{Ok(())}
}
