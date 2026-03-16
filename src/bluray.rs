use std::{error::Error, ffi::CStr};
use std::fs::File;
use std::io::Write;
use libbluray_sys::{
    bd_open, bd_get_titles, bd_select_title, bd_get_title_size, bd_tell, bd_read,
    bd_get_disc_info, bd_get_main_title, bd_get_title_info, bd_free_title_info,
};

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

pub fn disc_info(bd_path: &str) -> Result<(), Box<dyn Error>> {
    let bd_path_c = format!("{}\0", bd_path);
    let device = CStr::from_bytes_with_nul(bd_path_c.as_bytes())?;

    unsafe {
        let bd = bd_open(device.as_ptr(), std::ptr::null());
        if bd.is_null() {
            return Err("Failed to open BluRay device".into());
        }

        let info = bd_get_disc_info(bd);
        if info.is_null() {
            return Err("Failed to get disc info".into());
        }
        let info = &*info;

        let disc_name = if info.disc_name.is_null() {
            String::new()
        } else {
            CStr::from_ptr(info.disc_name).to_string_lossy().into_owned()
        };
        let udf_volume_id = if info.udf_volume_id.is_null() {
            String::new()
        } else {
            CStr::from_ptr(info.udf_volume_id).to_string_lossy().into_owned()
        };
        let disc_id: String = info.disc_id.iter().map(|b| format!("{:02x}", b)).collect();

        println!("Blu-Ray Info:");
        println!("  Volume Identifier: {}", udf_volume_id);
        println!("  Disc Name: {}", disc_name);
        println!("  Disc Id: {}", disc_id);
        println!("  First Play supported: {}", info.first_play_supported != 0);
        println!("  Top menu supported: {}", info.top_menu_supported != 0);
        println!("  HDMV titles: {}", info.num_hdmv_titles);
        println!("  BD-J titles: {}", info.num_bdj_titles);
        println!("  Unsupported titles: {}", info.num_unsupported_titles);

        println!("BD-J:");
        println!("  BD-J detected: {}", info.bdj_detected != 0);
        if info.bdj_detected != 0 {
            println!("  Java VM found: {}", info.libjvm_detected != 0);
            println!("  BD-J handled: {}", info.bdj_handled != 0);
            let org_id: String = info.bdj_org_id.iter().map(|b| format!("{:02x}", *b as u8)).collect();
            let bdj_disc_id: String = info.bdj_disc_id.iter().map(|b| format!("{:02x}", *b as u8)).collect();
            println!("  BD-J organization id: {}", org_id);
            println!("  BD-J disc id: {}", bdj_disc_id);
        }

        println!("AACS:");
        println!("  AACS detected: {}", info.aacs_detected != 0);
        if info.aacs_detected != 0 {
            println!("  libaacs detected: {}", info.libaacs_detected != 0);
            println!("  AACS MKB Version: {}", info.aacs_mkbv);
            println!("  AACS handled: {}", info.aacs_handled != 0);
            if info.aacs_handled == 0 {
                println!("  AACS Error code: {}", info.aacs_error_code);
            }
        }

        println!("BD+:");
        println!("  BD+ detected: {}", info.bdplus_detected != 0);
        if info.bdplus_detected != 0 {
            println!("  libbdplus detected: {}", info.libbdplus_detected != 0);
            println!("  BD+ generation: {}", info.bdplus_gen);
            println!("  BD+ release date: {}", info.bdplus_date);
            println!("  BD+ handled: {}", info.bdplus_handled != 0);
        }

        let mode_pref = if info.initial_output_mode_preference != 0 { "3D" } else { "2D" };
        println!("Application Info:");
        println!("  Initial mode preference: {}", mode_pref);
        println!("  3D content exists: {}", info.content_exist_3D != 0);
        println!("  Video format: {}", info.video_format);
        println!("  Frame rate: {}", info.frame_rate);
        println!("  Initial dynamic range: {}", info.initial_dynamic_range_type);
        let provider_data: String = info.provider_data.iter().map(|b| format!("{:02x}", b)).collect();
        println!("  Provider data: {}", provider_data);
    }

    Ok(())
}

pub fn list_titles(bd_path: &str) -> Result<(), Box<dyn Error>> {
    let bd_path_c = format!("{}\0", bd_path);
    let device = CStr::from_bytes_with_nul(bd_path_c.as_bytes())?;

    unsafe {
        let bd = bd_open(device.as_ptr(), std::ptr::null());
        if bd.is_null() {
            return Err("Failed to open BluRay device".into());
        }

        let total_titles = bd_get_titles(bd, 3, 10);
        let main_title = bd_get_main_title(bd);

        println!("Main title: {}", main_title);

        for i in 0..total_titles {
            let info = bd_get_title_info(bd, i, 0);
            if info.is_null() {
                continue;
            }
            let info_ref = &*info;
            // duration is in 90kHz ticks
            let duration_secs = info_ref.duration / 90_000;
            let hours = duration_secs / 3600;
            let minutes = (duration_secs % 3600) / 60;
            let secs = duration_secs % 60;
            println!("Title: {}, Duration: {:02}:{:02}:{:02}", info_ref.idx, hours, minutes, secs);
            bd_free_title_info(info);
        }
    }

    Ok(())
}
