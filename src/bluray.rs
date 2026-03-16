use std::{error::Error, ffi::CStr};
use std::fs::File;
use std::io::Write;
use libbluray_sys::{bd_open, bd_get_titles, bd_select_title, bd_get_title_size, bd_tell, bd_read};

pub fn read_title(title: u32, out_path: &str, bd_path: &str) -> Result<u64, Box<dyn Error>> {
    let bd_path = format!("{}\0", bd_path);
    let device = CStr::from_bytes_with_nul(
        bd_path.as_bytes()
    ).unwrap();

    let mut total_read_bytes: u64 = 0;
    unsafe {
        let bd = bd_open(device.as_ptr(), std::ptr::null());
        if bd.is_null() {
            return Err("Failed to open BluRay device".into());
        }

        bd_get_titles(bd, 3, 10);
        bd_select_title(bd, title);

        let title_size = bd_get_title_size(bd);
        let mut current_pos = bd_tell(bd);

        let mut file = File::create(out_path)?;

        loop {
            let chunk_size: u64 = 10_000_000;
            let size = chunk_size.min(title_size - current_pos) as i32;

            let mut buffer = vec![0u8; size as usize];
            let read_size = bd_read(bd, buffer.as_mut_ptr(), size);
            total_read_bytes += read_size as u64;

            current_pos = bd_tell(bd);
            file.write_all(&buffer[..read_size as usize])?;

            if read_size < 10_000_000 || size == 0 {
                println!("Finished reading");
                break;
            }
        }
    }

    Ok(total_read_bytes)
}
