//! The general pasteboard. Unlike Wayland and X11, the pasteboard server
//! keeps a copy, so clipt can exit as soon as it has handed the image over.

use anyhow::{Result, ensure};
use objc2_app_kit::{NSBitmapImageRep, NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF};
use objc2_foundation::NSData;

pub fn copy_png(png: &[u8]) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let data = NSData::with_bytes(png);
    ensure!(pb.setData_forType(Some(&data), unsafe { NSPasteboardTypePNG }), "the pasteboard refused the image");
    // TIFF too, as the system's own screenshots offer it: some apps read
    // nothing else.
    if let Some(tiff) = NSBitmapImageRep::imageRepWithData(&data).and_then(|r| r.TIFFRepresentation()) {
        pb.setData_forType(Some(&tiff), unsafe { NSPasteboardTypeTIFF });
    }
    Ok(())
}
