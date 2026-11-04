//! The Win32 clipboard. Like the macOS pasteboard, Windows keeps its own
//! copy, so clipt can exit once it has handed the image over.

use super::wide;
use crate::image::Image;
use anyhow::{Result, bail};
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_DIB;

/// The image as a DIB, which every Windows program can paste (and from
/// which Windows makes the older bitmap formats), and as PNG, which
/// browsers, Office and chat apps prefer.
pub fn copy(img: &Image, png: &[u8]) -> Result<()> {
    // Another program may have it open for a moment.
    let mut tries = 0;
    while unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
        tries += 1;
        if tries == 20 {
            bail!("the clipboard is busy");
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let r = (|| {
        if unsafe { EmptyClipboard() } == 0 {
            bail!("emptying the clipboard failed");
        }
        put(CF_DIB as u32, &dib(img))?;
        let fmt = unsafe { RegisterClipboardFormatW(wide("PNG").as_ptr()) };
        if fmt != 0 {
            put(fmt, png)?;
        }
        Ok(())
    })();
    unsafe { CloseClipboard() };
    r
}

fn put(format: u32, data: &[u8]) -> Result<()> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, data.len());
        if h.is_null() {
            bail!("out of memory for the clipboard");
        }
        let p = GlobalLock(h);
        if p.is_null() {
            GlobalFree(h);
            bail!("GlobalLock failed");
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), p.cast(), data.len());
        GlobalUnlock(h);
        // On success the clipboard owns the memory.
        if SetClipboardData(format, h).is_null() {
            GlobalFree(h);
            bail!("the clipboard refused the image");
        }
    }
    Ok(())
}

/// A packed 32-bit DIB: the header, then BGRX rows bottom to top, the order
/// programs reading CF_DIB expect.
fn dib(img: &Image) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: img.width as i32,
        biHeight: img.height as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: img.width * img.height * 4,
        ..unsafe { std::mem::zeroed() }
    };
    let mut out = Vec::with_capacity(size_of::<BITMAPINFOHEADER>() + img.data.len());
    out.extend_from_slice(unsafe {
        std::slice::from_raw_parts((&header as *const BITMAPINFOHEADER).cast::<u8>(), size_of::<BITMAPINFOHEADER>())
    });
    for row in img.data.chunks_exact(img.width as usize * 4).rev() {
        for px in row.as_chunks::<4>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn dib_is_bottom_up_bgr() {
        let mut img = crate::image::Image::new(1, 2);
        img.data = vec![1, 2, 3, 255, 4, 5, 6, 255];
        let d = super::dib(&img);
        assert_eq!(d.len(), 40 + 8);
        assert_eq!(&d[40..], &[6, 5, 4, 255, 3, 2, 1, 255]);
    }
}
