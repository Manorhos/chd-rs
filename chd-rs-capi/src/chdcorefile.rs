use std::any::Any;
use std::os::raw::c_void;
use std::io::{Read, Seek, SeekFrom};
use crate::chdcorefile_sys::*;
use crate::SeekRead;

pub struct CoreFile(pub(crate) *mut core_file);

impl SeekRead for CoreFile {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Read for CoreFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let res = unsafe {
            core_fread(self.0, buf.as_mut_ptr() as *mut c_void, buf.len() as size_t)
        };
        Ok(res as usize)
    }
}

impl Seek for CoreFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let (off, set) = match pos {
            SeekFrom::Start(off) => { (off as i64, 0) } // SEEK_SET
            SeekFrom::End(off) => { (off, 2) } // SEEK_END
            SeekFrom::Current(off) => { (off, 1) } // SEEK_CUR
        };
        let res = unsafe {
            core_fseek(self.0, off as size_t, set)
        };
        Ok(res as u64)
    }
}


#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::{Read, Write};
    use crate::chdcorefile::CoreFile;
    use crate::chdcorefile_sys::core_fopen;

    #[test]
    fn chdcorefile_read() {
        let mut f = File::create("test.txt").unwrap();
        f.write_all(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]).unwrap();
        f.flush().unwrap();
        drop(f);

        let file = unsafe {
            core_fopen(b"test.txt\0".as_ptr() as *const std::os::raw::c_char)
        };
        let mut file = CoreFile(file);
        let mut buf = [0u8; 10];
        file.read_exact(&mut buf).unwrap();
        assert_eq!(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9], &buf);
    }
}

impl Drop for CoreFile {
    fn drop(&mut self) {
        unsafe {
            core_fclose(self.0)
        }
    }
}