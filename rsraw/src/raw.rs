use std::{borrow::Cow, marker::PhantomData, path::Path, ptr::NonNull};

use chrono::{DateTime, Local, TimeZone};
use rsraw_sys as sys;

use crate::{
    err::{Error, Result},
    processed::ProcessedImage,
    GpsInfo, LensInfo, ThumbnailImage, Thumbnails,
};

pub type BitDepth = u32;

pub const BIT_DEPTH_8: BitDepth = 8;
pub const BIT_DEPTH_16: BitDepth = 16;

/// Highlight handling, LibRaw's `params.highlight` (dcraw `-H`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HighlightMode {
    /// Clip all channels to the saturation point (default). Partially clipped
    /// highlights can take on a colour cast, typically magenta.
    #[default]
    Clip,
    /// Leave highlights unclipped, in various shades of pink.
    Unclip,
    /// Blend clipped and unclipped values for a gradual fade to white.
    Blend,
    /// Reconstruct highlights; 1 favours whites, 7 favours colours (dcraw `-H 3..9`).
    Rebuild(u8),
}

/// A LibRaw decoder reading from a caller-owned buffer.
///
/// LibRaw keeps a pointer to the buffer passed to [`RawImage::open`] and reads
/// from it during [`unpack`](Self::unpack) and thumbnail extraction, so the
/// image borrows the buffer for its whole lifetime:
///
/// ```compile_fail
/// let image = {
///     let buf = std::fs::read("photo.nef").unwrap();
///     rsraw::RawImage::open(&buf).unwrap()
/// }; // error: `buf` does not live long enough
/// ```
pub struct RawImage<'a> {
    raw_data: NonNull<sys::libraw_data_t>,
    _buf: PhantomData<&'a [u8]>,
}

// LibRaw has no global mutable state; one instance is only touched through
// `&mut self` (or read through `&self`), and the borrowed buffer is `Sync`.
unsafe impl Sync for RawImage<'_> {}

unsafe impl Send for RawImage<'_> {}

impl<'a> RawImage<'a> {
    pub fn open(buf: &'a [u8]) -> Result<Self> {
        let raw_data =
            NonNull::new(unsafe { sys::libraw_init(0) }).ok_or(Error::UnsufficientMemory)?;
        // Construct `Self` before anything can fail so `Drop` closes the handle.
        let image = Self {
            raw_data,
            _buf: PhantomData,
        };
        Error::check(unsafe {
            sys::libraw_open_buffer(image.raw_data.as_ptr(), buf.as_ptr() as *const _, buf.len())
        })?;
        Ok(image)
    }

    pub fn open_file(path: &Path) -> Result<Self> {
        let raw_data =
            NonNull::new(unsafe { sys::libraw_init(0) }).ok_or(Error::UnsufficientMemory)?;
        let image = Self {
            raw_data,
            _buf: PhantomData,
        };

        let c_str = std::ffi::CString::new(path.as_os_str().to_string_lossy().as_ref())
            .map_err(|_| Error::Io)?;

        Error::check(unsafe { sys::libraw_open_file(image.raw_data.as_ptr(), c_str.as_ptr()) })?;
        Ok(image)
    }

    fn ptr(&self) -> *mut sys::libraw_data_t {
        self.raw_data.as_ptr()
    }

    /// Raw pointer to the underlying LibRaw handle, for settings this wrapper
    /// does not expose.
    ///
    /// # Safety
    /// LibRaw trusts every field of `libraw_data_t`, including internal
    /// pointers. Callers must only change plain processing parameters and must
    /// not keep the pointer beyond the lifetime of `self`.
    pub unsafe fn as_mut_ptr(&mut self) -> *mut sys::libraw_data_t {
        self.ptr()
    }

    pub fn unpack(&mut self) -> Result<()> {
        Error::check(unsafe { sys::libraw_unpack(self.ptr()) })
    }

    pub fn extract_thumbs(&mut self) -> Result<Vec<ThumbnailImage>> {
        let mut thumbs = Thumbnails::new();
        for i in 0..self.as_ref().thumbs_list.thumbcount {
            Error::check(unsafe { sys::libraw_unpack_thumb_ex(self.ptr(), i) })?;
            let thumb = &self.as_ref().thumbnail;
            // Some loaders (e.g. Kodak) report success without allocating.
            if thumb.thumb.is_null() || thumb.tlength == 0 {
                continue;
            }
            let thumb_image = ThumbnailImage {
                format: thumb.tformat.into(),
                width: thumb.twidth as _,
                height: thumb.theight as _,
                colors: thumb.tcolors as _,
                data: unsafe {
                    std::slice::from_raw_parts(thumb.thumb as *const u8, thumb.tlength as _)
                        .to_vec()
                },
            };
            thumbs.append(thumb_image);
        }
        Ok(thumbs.into_inner())
    }

    /// Enables or disables half-size output (2x2 pixel binning, no
    /// demosaicing), which is much faster. Takes effect at the next
    /// [`process`](Self::process).
    pub fn set_half_size(&mut self, enable: bool) {
        unsafe { (*self.ptr()).params.half_size = enable.into() };
    }

    /// Output transfer curve as a `(power, toe slope)` pair, e.g.
    /// `(1.0 / 2.4, 12.92)` for sRGB. LibRaw defaults to BT.709 `(0.45, 4.5)`.
    pub fn set_gamma(&mut self, power: f64, slope: f64) {
        unsafe {
            (*self.ptr()).params.gamm[0] = power;
            (*self.ptr()).params.gamm[1] = slope;
        }
    }

    /// Disables LibRaw's automatic brightening of the output histogram.
    pub fn set_no_auto_bright(&mut self, disable: bool) {
        unsafe { (*self.ptr()).params.no_auto_bright = disable.into() };
    }

    /// How to treat pixels where one or more channels hit the sensor's
    /// saturation point. Takes effect at the next [`process`](Self::process).
    pub fn set_highlight_mode(&mut self, mode: HighlightMode) {
        let value = match mode {
            HighlightMode::Clip => 0,
            HighlightMode::Unclip => 1,
            HighlightMode::Blend => 2,
            HighlightMode::Rebuild(level) => 2 + i32::from(level.clamp(1, 7)),
        };
        unsafe { (*self.ptr()).params.highlight = value };
    }

    /// Upper bound on raw data memory LibRaw may allocate in
    /// [`unpack`](Self::unpack). LibRaw's default is 2048 MB.
    pub fn set_max_raw_memory_mb(&mut self, mb: u32) {
        unsafe { (*self.ptr()).rawparams.max_raw_memory_mb = mb };
    }

    pub fn width(&self) -> u32 {
        self.as_ref().sizes.width as _
    }

    pub fn height(&self) -> u32 {
        self.as_ref().sizes.height as _
    }

    pub fn pixels(&self) -> u32 {
        self.width() * self.height()
    }

    pub fn colors(&self) -> i32 {
        self.as_ref().idata.colors
    }

    pub fn iso_speed(&self) -> u32 {
        self.as_ref().other.iso_speed as _
    }

    pub fn shutter(&self) -> f32 {
        self.as_ref().other.shutter as _
    }

    pub fn aperture(&self) -> f32 {
        self.as_ref().other.aperture as _
    }

    pub fn focal_len(&self) -> f32 {
        self.as_ref().other.focal_len as _
    }

    pub fn datetime(&self) -> Option<DateTime<Local>> {
        let ts = self.as_ref().other.timestamp;
        Local.timestamp_opt(i64::from(ts), 0).single()
    }

    pub fn gps(&self) -> GpsInfo {
        self.as_ref().other.parsed_gps.into()
    }

    pub fn artist(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().other.artist[0] as *const _).to_string_lossy()
        }
    }

    pub fn desc(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().other.desc[0] as *const _).to_string_lossy()
        }
    }

    pub fn make(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().idata.make[0] as *const _).to_string_lossy()
        }
    }

    pub fn model(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().idata.model[0] as *const _).to_string_lossy()
        }
    }

    pub fn normalized_make(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().idata.normalized_make[0] as *const _)
                .to_string_lossy()
        }
    }

    pub fn normalized_model(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().idata.normalized_model[0] as *const _)
                .to_string_lossy()
        }
    }

    pub fn software(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().idata.software[0] as *const _).to_string_lossy()
        }
    }

    pub fn raw_count(&self) -> u32 {
        self.as_ref().idata.raw_count as _
    }

    pub fn dng_version(&self) -> u32 {
        self.as_ref().idata.dng_version as _
    }

    pub fn lens_info(&self) -> LensInfo {
        (&self.as_ref().lens).into()
    }

    pub fn full_info(&self) -> FullRawInfo {
        FullRawInfo {
            width: self.width(),
            height: self.height(),
            colors: self.colors(),
            iso_speed: self.iso_speed(),
            shutter: self.shutter(),
            aperture: self.aperture(),
            focal_len: self.focal_len(),
            datetime: self.datetime(),
            gps: self.gps(),
            artist: self.artist().to_string(),
            desc: self.desc().trim().into(),
            make: self.make().to_string(),
            model: self.model().to_string(),
            normalized_make: self.normalized_make().to_string(),
            normalized_model: self.normalized_model().to_string(),
            software: self.software().to_string(),
            raw_count: self.raw_count(),
            dng_version: self.dng_version(),
            lens_info: self.lens_info(),
        }
    }

    /// If possible, use the white balance from the camera.
    /// If not available, Auto-WB is used.
    pub fn set_use_camera_wb(&mut self, enable: bool) {
        unsafe {
            (*self.ptr()).params.use_camera_wb = enable.into();
        }
    }

    /// On by default, call if you want to force use of DNG embedded matrix.
    pub fn set_use_camera_matrix(&mut self, enable: bool) {
        unsafe {
            (*self.ptr()).params.use_camera_matrix = if enable { 3 } else { 0 };
        }
    }

    /// Demosaic and colour-convert into an in-memory bitmap with `D` bits per
    /// sample. Only [`BIT_DEPTH_8`] and [`BIT_DEPTH_16`] are accepted; any
    /// other depth is rejected at compile time (LibRaw would otherwise write
    /// past the end of the buffer it allocates):
    ///
    /// ```compile_fail
    /// let buf = std::fs::read("photo.nef").unwrap();
    /// let mut image = rsraw::RawImage::open(&buf).unwrap();
    /// let _ = image.process::<12>(); // error: only 8 or 16 bits
    /// ```
    pub fn process<const D: BitDepth>(&mut self) -> Result<ProcessedImage<D>> {
        const {
            assert!(
                D == BIT_DEPTH_8 || D == BIT_DEPTH_16,
                "process::<D>() only supports BIT_DEPTH_8 and BIT_DEPTH_16"
            )
        };
        unsafe { (*self.ptr()).params.output_bps = D as i32 };
        Error::check(unsafe { sys::libraw_dcraw_process(self.ptr()) })?;

        let mut result = 0i32;
        let processed = unsafe { sys::libraw_dcraw_make_mem_image(self.ptr(), &mut result) };
        Error::check(result)?;
        if processed.is_null() {
            return Err(Error::UnsufficientMemory);
        }
        Ok(unsafe { ProcessedImage::from_raw(processed) })
    }
}

impl Drop for RawImage<'_> {
    fn drop(&mut self) {
        unsafe { sys::libraw_close(self.ptr()) }
    }
}

impl AsRef<sys::libraw_data_t> for RawImage<'_> {
    fn as_ref(&self) -> &sys::libraw_data_t {
        unsafe { self.raw_data.as_ref() }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FullRawInfo {
    pub width: u32,
    pub height: u32,
    pub colors: i32,
    pub iso_speed: u32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal_len: f32,
    pub datetime: Option<DateTime<Local>>,
    pub gps: GpsInfo,
    pub artist: String,
    pub desc: String,
    pub make: String,
    pub model: String,
    pub normalized_make: String,
    pub normalized_model: String,
    pub software: String,
    pub raw_count: u32,
    pub dng_version: u32,
    pub lens_info: LensInfo,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rsraw_sys::{
        LibRaw_camera_mounts_LIBRAW_MOUNT_Nikon_Z, LibRaw_camera_mounts_LIBRAW_MOUNT_Sony_E,
    };

    use super::*;
    use crate::{lens::FocusType, processed::ImageFormat, Mounts};

    fn get_test_assets_path() -> PathBuf {
        let root: PathBuf = std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("must get manifest dir")
            .into();
        root.join("tests/assets")
    }

    #[test]
    fn test_raw_metadata() {
        let assets = get_test_assets_path();

        let test_cases = [
            (
                "test-z8.NEF",
                FullRawInfo {
                    width: 8280,
                    height: 5520,
                    colors: 3,
                    iso_speed: 250,
                    shutter: 1.0 / 100.0,
                    aperture: 3.5,
                    focal_len: 105.,
                    datetime: Local.with_ymd_and_hms(2024, 11, 4, 20, 11, 38).single(),
                    // Z8 NEF has GPS EXIF tags present but coordinates are
                    // zeroed (no fix). LibRaw still sets gpsparsed = 1, but
                    // since all coords are zero the result equals default.
                    gps: GpsInfo::default(),
                    artist: "HEXILEE".into(),
                    desc: "".into(),
                    make: "Nikon".into(),
                    model: "Z 8".into(),
                    normalized_make: "Nikon".into(),
                    normalized_model: "Z 8".into(),
                    software: "Ver.02.00".into(),
                    raw_count: 1,
                    dng_version: 0,
                    lens_info: LensInfo {
                        min_focal: 105.0,
                        max_focal: 105.0,
                        max_aperture_at_min_focal: 2.8,
                        max_aperture_at_max_focal: 2.8,
                        lens_make: "NIKON".into(),
                        lens_name: "NIKKOR Z MC 105mm f/2.8 VR S".into(),
                        lens_serial: "20044280".into(),
                        internal_lens_serial: "".into(),
                        focal_length_in_35mm_format: 105,
                        mounts: Mounts::from(LibRaw_camera_mounts_LIBRAW_MOUNT_Nikon_Z).to_string(),
                        focus_type: FocusType::Prime,
                        feture_pre: "AF".into(),
                        feture_suf: "".into(),
                    },
                },
            ),
            (
                "test-a7rm4.ARW",
                FullRawInfo {
                    width: 9568,
                    height: 6376,
                    colors: 3,
                    iso_speed: 320,
                    shutter: 1.0 / 500.0,
                    aperture: 4.0,
                    focal_len: 40.,
                    datetime: Local.with_ymd_and_hms(2023, 11, 17, 13, 0, 13).single(),
                    gps: Default::default(),
                    artist: "hexilee".into(),
                    desc: "".into(),
                    make: "Sony".into(),
                    model: "ILCE-7RM4".into(),
                    normalized_make: "Sony".into(),
                    normalized_model: "ILCE-7RM4".into(),
                    software: "ILCE-7RM4 v1.20".into(),
                    raw_count: 1,
                    dng_version: 0,
                    lens_info: LensInfo {
                        min_focal: 40.0,
                        max_focal: 40.0,
                        max_aperture_at_min_focal: 1.4,
                        max_aperture_at_max_focal: 1.4,
                        lens_make: "".into(),
                        lens_name: "40mm F1.4 DG HSM | Art 018".into(),
                        lens_serial: "".into(),
                        internal_lens_serial: "".into(),
                        focal_length_in_35mm_format: 40,
                        mounts: Mounts::from(LibRaw_camera_mounts_LIBRAW_MOUNT_Sony_E).to_string(),
                        focus_type: FocusType::Prime,
                        feture_pre: "".into(),
                        feture_suf: "".into(),
                    },
                },
            ),
        ];
        for (file, expected) in test_cases {
            let path = assets.join(file);
            let data = std::fs::read(path).unwrap();
            let raw_image = RawImage::open(&data).expect("opened");
            let full_info = raw_image.full_info();
            assert_eq!(full_info, expected);
        }
    }

    #[test]
    fn test_thumbnails() {
        let assets = get_test_assets_path();
        let test_cases = [("test-z8.NEF",), ("test-a7rm4.ARW",)];
        for (file,) in test_cases {
            let path = assets.join(file);
            println!("{path:?}");
            let data = std::fs::read(path).unwrap();
            let mut raw_image = RawImage::open(&data).expect("opened");
            let thumbs = raw_image.extract_thumbs().expect("extracted");
            println!("{:?}", thumbs);
        }
    }

    #[test]
    fn test_processed() {
        let assets = get_test_assets_path();
        let test_cases = [
            (
                "test-z8.NEF",
                8280,
                5520,
                ImageFormat::Bitmap,
                3,
                16,
                274233600,
            ),
            (
                "test-a7rm4.ARW",
                9568,
                6376,
                ImageFormat::Bitmap,
                3,
                16,
                366033408,
            ),
        ];
        for (file, width, height, format, colors, bits, data_size) in test_cases {
            let path = assets.join(file);
            println!("{path:?}");
            let data = std::fs::read(path).unwrap();
            let mut raw_image = RawImage::open(&data).expect("opened");
            raw_image.unpack().expect("unpacked");
            let image = raw_image.process::<BIT_DEPTH_16>().expect("decoded");
            assert_eq!(image.width(), width);
            assert_eq!(image.height(), height);
            assert_eq!(image.image_format(), format);
            assert_eq!(image.colors(), colors);
            assert_eq!(image.bits(), bits);
            assert_eq!(image.data_size(), data_size);
        }
    }
}
