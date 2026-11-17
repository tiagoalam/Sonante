//! Microsoft built-in WIC codecs, private fixed-size buffers, no file/URL API.
use super::{png, Kind, Plan, LIMIT};
use windows::{
    core::Interface,
    Win32::{
        Graphics::Imaging::*,
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, StructuredStorage::IPropertyBag2,
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
        },
    },
};
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
unsafe fn memory(
    factory: &IWICImagingFactory,
    buffer: &mut [u8],
) -> windows::core::Result<IWICStream> {
    let stream = unsafe { factory.CreateStream()? };
    // Generated InitializeFromMemory takes &[u8], although the COM stream can
    // write. Use its GENERATED vtable with a pointer derived from &mut instead
    // of allowing a foreign writer to alias a Rust shared byte reference.
    // Callers keep storage fixed and inaccessible until every COM ref drops.
    unsafe {
        (Interface::vtable(&stream).InitializeFromMemory)(
            Interface::as_raw(&stream),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
        .ok()?;
    }
    Ok(stream)
}
pub(super) fn render(bytes: &[u8], plan: Plan) -> Option<Vec<u8>> {
    let mut input = bytes.to_vec();
    let mut output = vec![0u8; LIMIT];
    (|| -> Option<()> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
            let _apartment = Apartment;
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let source = memory(&factory, &mut input).ok()?;
            let container = match plan.kind {
                Kind::Png => GUID_ContainerFormatPng,
                Kind::Jpeg => GUID_ContainerFormatJpeg,
            };
            let decoder = factory
                .CreateDecoder(&container, &GUID_VendorMicrosoftBuiltIn)
                .ok()?;
            let vendor = decoder.GetDecoderInfo().ok()?.GetVendorGUID().ok()?;
            if vendor != GUID_VendorMicrosoft && vendor != GUID_VendorMicrosoftBuiltIn {
                return None;
            }
            decoder
                .Initialize(&source, WICDecodeMetadataCacheOnDemand)
                .ok()?;
            if decoder.GetContainerFormat().ok()? != container || decoder.GetFrameCount().ok()? != 1
            {
                return None;
            }
            let frame = decoder.GetFrame(0).ok()?;
            let (mut width, mut height) = (0, 0);
            frame.GetSize(&mut width, &mut height).ok()?;
            if (width, height) != (plan.width, plan.height) {
                return None;
            }
            let scaler = factory.CreateBitmapScaler().ok()?;
            scaler
                .Initialize(
                    &frame,
                    plan.thumb_width,
                    plan.thumb_height,
                    WICBitmapInterpolationModeFant,
                )
                .ok()?;
            let converter = factory.CreateFormatConverter().ok()?;
            converter
                .Initialize(
                    &scaler,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapDitherTypeNone,
                    None::<&IWICPalette>,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .ok()?;
            let target = memory(&factory, &mut output).ok()?;
            let encoder = factory
                .CreateEncoder(&GUID_ContainerFormatPng, &GUID_VendorMicrosoftBuiltIn)
                .ok()?;
            let vendor = encoder.GetEncoderInfo().ok()?.GetVendorGUID().ok()?;
            if vendor != GUID_VendorMicrosoft && vendor != GUID_VendorMicrosoftBuiltIn {
                return None;
            }
            encoder.Initialize(&target, WICBitmapEncoderNoCache).ok()?;
            let mut encoded = None;
            encoder
                .CreateNewFrame(&mut encoded, std::ptr::null_mut())
                .ok()?;
            let encoded = encoded?;
            encoded.Initialize(None::<&IPropertyBag2>).ok()?;
            encoded.SetSize(plan.thumb_width, plan.thumb_height).ok()?;
            let mut format = GUID_WICPixelFormat32bppBGRA;
            encoded.SetPixelFormat(&mut format).ok()?;
            if format != GUID_WICPixelFormat32bppBGRA {
                return None;
            }
            encoded.WriteSource(&converter, std::ptr::null()).ok()?;
            encoded.Commit().ok()?;
            encoder.Commit().ok()?;
            Some(())
        }
    })()?;
    // COM streams/encoders/decoders/apartment were all dropped inside the scope
    // above. No interface or borrowed buffer pointer escapes this function.
    let (end, w, h) = png(&output)?;
    if (w, h) != (plan.thumb_width, plan.thumb_height) {
        return None;
    }
    output.truncate(end);
    Some(output)
}
