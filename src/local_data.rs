use std::{io, path::PathBuf};

pub(crate) use platform::{dpapi, move_file};

pub(crate) fn user_data_dir() -> io::Result<PathBuf> {
    let directory = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("Windows local app data directory is unavailable"))?;
    Ok(directory.join("VOIDCHATS").join("Native"))
}

#[cfg(windows)]
mod platform {
    use std::{io, os::windows::ffi::OsStrExt, path::Path, ptr, slice};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData},
        Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
    };
    use zeroize::{Zeroize, Zeroizing};

    pub fn dpapi(bytes: &[u8], protect: bool) -> io::Result<Zeroizing<Vec<u8>>> {
        let input = CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_ptr().cast_mut() };
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() };
        // SAFETY: the input lives through the call. DPAPI returns a LocalAlloc buffer on success.
        let ok = unsafe {
            if protect {
                CryptProtectData(&input, ptr::null(), ptr::null(), ptr::null(), ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN, &mut output)
            } else {
                CryptUnprotectData(&input, ptr::null_mut(), ptr::null(), ptr::null(), ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN, &mut output)
            }
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if output.pbData.is_null() {
            return Err(io::Error::other("DPAPI returned no data"));
        }
        // SAFETY: DPAPI owns this non-null buffer until LocalFree. Wipe its contents before freeing it.
        unsafe {
            let data = slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
            let copy = Zeroizing::new(data.to_vec());
            data.zeroize();
            LocalFree(output.pbData.cast());
            Ok(copy)
        }
    }

    pub fn move_file(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
        let from: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
        let to: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
        let flags = MOVEFILE_WRITE_THROUGH | if replace { MOVEFILE_REPLACE_EXISTING } else { 0 };
        // SAFETY: both Windows path buffers are NUL-terminated and remain live during the call.
        let ok = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) };
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::{io, path::Path};
    use zeroize::Zeroizing;

    pub fn dpapi(_: &[u8], _: bool) -> io::Result<Zeroizing<Vec<u8>>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Windows DPAPI is required"))
    }

    pub fn move_file(_: &Path, _: &Path, _: bool) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Windows storage is required"))
    }
}
