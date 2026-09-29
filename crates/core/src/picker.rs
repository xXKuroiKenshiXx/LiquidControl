//! Selector de archivos nativo de Windows (`IFileOpenDialog`).

use std::ffi::c_void;
use std::path::PathBuf;

use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, w};

use crate::error::{AppError, AppResult};
use crate::util::com;

/// Abre el diálogo "Abrir" filtrado a imágenes. `owner` es el HWND de la ventana
/// (0 = sin dueño). Devuelve `None` si el usuario cancela.
pub fn pick_image(owner: isize) -> AppResult<Option<PathBuf>> {
    com::sta(move || unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let filters = [COMDLG_FILTERSPEC {
            pszName: w!("Imágenes"),
            pszSpec: w!("*.jpg;*.jpeg;*.jfif;*.png;*.bmp;*.gif;*.webp;*.heic;*.heif;*.avif;*.jxl;*.tif;*.tiff"),
        }];
        dialog.SetFileTypes(&filters)?;
        dialog.SetTitle(w!("Elegí una imagen"))?;
        dialog.SetOptions(dialog.GetOptions()? | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM)?;
        let owner = (owner != 0).then_some(HWND(owner as *mut c_void));
        if let Err(err) = dialog.Show(owner) {
            return if err.code() == HRESULT::from_win32(ERROR_CANCELLED.0) { Ok(None) } else { Err(err.into()) };
        }
        let raw = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = raw.to_string().map_err(|_| AppError::failed("La ruta elegida no es válida."));
        CoTaskMemFree(Some(raw.0 as *const _));
        Ok(Some(PathBuf::from(path?)))
    })
}

/// Abre el diálogo "Abrir" para elegir uno o varios archivos de cualquier
/// tipo (para mandarlos por AirDrop). Lista vacía = el usuario canceló.
pub fn pick_files(owner: isize) -> AppResult<Vec<PathBuf>> {
    com::sta(move || unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetTitle(w!("Elegí los archivos para enviar"))?;
        dialog.SetOptions(dialog.GetOptions()? | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM | FOS_ALLOWMULTISELECT)?;
        let owner = (owner != 0).then_some(HWND(owner as *mut c_void));
        if let Err(err) = dialog.Show(owner) {
            return if err.code() == HRESULT::from_win32(ERROR_CANCELLED.0) { Ok(Vec::new()) } else { Err(err.into()) };
        }
        let items = dialog.GetResults()?;
        let mut paths = Vec::new();
        for index in 0..items.GetCount()? {
            let raw = items.GetItemAt(index)?.GetDisplayName(SIGDN_FILESYSPATH)?;
            if let Ok(path) = raw.to_string() {
                paths.push(PathBuf::from(path));
            }
            CoTaskMemFree(Some(raw.0 as *const _));
        }
        Ok(paths)
    })
}
