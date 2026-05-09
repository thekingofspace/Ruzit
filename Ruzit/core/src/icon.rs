use std::path::Path;

#[cfg(windows)]
pub fn embed_icon(exe_path: &Path, ico_bytes: &[u8]) -> Result<(), String> {
    win::embed_icon(exe_path, ico_bytes)
}

#[cfg(not(windows))]
pub fn embed_icon(_exe_path: &Path, _ico_bytes: &[u8]) -> Result<(), String> {
    Err("icon embedding is only supported on Windows".to_string())
}

#[cfg(windows)]
mod win {
    use std::ffi::OsStr;
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::System::LibraryLoader::{
        BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW,
    };

    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    const LANG_NEUTRAL: u16 = 0;

    pub fn embed_icon(exe_path: &Path, ico_bytes: &[u8]) -> Result<(), String> {
        let entries = parse_ico(ico_bytes)?;
        if entries.is_empty() {
            return Err("ICO file contains no images".into());
        }

        let path_w: Vec<u16> = OsStr::new(exe_path).encode_wide().chain(once(0)).collect();

        unsafe {
            let handle = BeginUpdateResourceW(path_w.as_ptr(), 0);
            if handle.is_null() {
                return Err("BeginUpdateResource failed".into());
            }

            let mut grp = Vec::with_capacity(6 + entries.len() * 14);
            grp.extend_from_slice(&[0u8, 0]);
            grp.extend_from_slice(&[1u8, 0]);
            grp.extend_from_slice(&(entries.len() as u16).to_le_bytes());

            for (i, e) in entries.iter().enumerate() {
                let id = (i as u16) + 1;

                grp.extend_from_slice(&e.meta);
                grp.extend_from_slice(&id.to_le_bytes());

                let ok = UpdateResourceW(
                    handle,
                    RT_ICON as usize as *const u16,
                    id as usize as *const u16,
                    LANG_NEUTRAL,
                    e.image.as_ptr() as *mut _,
                    e.image.len() as u32,
                );
                if ok == 0 {
                    let _ = EndUpdateResourceW(handle, 1);
                    return Err(format!("UpdateResource RT_ICON {id} failed"));
                }
            }

            let ok = UpdateResourceW(
                handle,
                RT_GROUP_ICON as usize as *const u16,
                1usize as *const u16,
                LANG_NEUTRAL,
                grp.as_ptr() as *mut _,
                grp.len() as u32,
            );
            if ok == 0 {
                let _ = EndUpdateResourceW(handle, 1);
                return Err("UpdateResource RT_GROUP_ICON failed".into());
            }

            let ok = EndUpdateResourceW(handle, 0);
            if ok == 0 {
                return Err("EndUpdateResource failed".into());
            }
        }

        Ok(())
    }

    struct Entry<'a> {
        meta: [u8; 12],
        image: &'a [u8],
    }

    fn parse_ico(bytes: &[u8]) -> Result<Vec<Entry<'_>>, String> {
        if bytes.len() < 6 {
            return Err("ICO file too small for header".into());
        }
        let icon_type = u16::from_le_bytes([bytes[2], bytes[3]]);
        if icon_type != 1 {
            return Err(format!("expected ICO file (type 1), got type {icon_type}"));
        }
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        if bytes.len() < 6 + count * 16 {
            return Err("ICO directory truncated".into());
        }

        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let off = 6 + i * 16;
            let bytes_in_res = u32::from_le_bytes([
                bytes[off + 8],
                bytes[off + 9],
                bytes[off + 10],
                bytes[off + 11],
            ]) as usize;
            let image_offset = u32::from_le_bytes([
                bytes[off + 12],
                bytes[off + 13],
                bytes[off + 14],
                bytes[off + 15],
            ]) as usize;
            if image_offset + bytes_in_res > bytes.len() {
                return Err(format!("ICO entry {i} points past EOF"));
            }
            let mut meta = [0u8; 12];
            meta.copy_from_slice(&bytes[off..off + 12]);
            out.push(Entry {
                meta,
                image: &bytes[image_offset..image_offset + bytes_in_res],
            });
        }
        Ok(out)
    }
}
