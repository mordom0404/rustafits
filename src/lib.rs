#[cfg(feature = "debug-pipeline")]
pub mod analysis;
#[cfg(not(feature = "debug-pipeline"))]
mod analysis;
mod annotate;
mod converter;
mod exif;
#[cfg(feature = "debug-pipeline")]
pub mod formats;
#[cfg(not(feature = "debug-pipeline"))]
mod formats;
mod output;
mod pipeline;
pub mod platesolving;
pub mod processing;
mod types;

pub use analysis::{
    AnalysisConfig, AnalysisResult, DetectionLevels, FastAnalysisResult, FastDetectTiming,
    FastStar, FitMethod, ImageAnalyzer, StageTiming, StarMetrics,
};
pub use annotate::{
    annotate_image, compute_annotations, create_annotation_layer, AnnotationConfig, ColorScheme,
    StarAnnotation,
};
pub use converter::ImageConverter;
/// The observational keywords of a FITS/XISF header, ready to be written into
/// the EXIF block of a JPEG.
pub use exif::ObservationalMetadata;
/// In-memory JPEG encoding (baseline, 4:2:0). Accepts RGB (3 ch) or RGBA (4 ch)
/// — alpha is discarded by the encoder, so callers must not pre-strip it.
pub use output::encode_jpeg;
/// [`encode_jpeg`] with an EXIF block built from the file's header.
pub use output::encode_jpeg_with_exif;
pub use output::{encode_png, OutputFormat, PngCompression};
pub use rayon::{ThreadPool, ThreadPoolBuilder};
pub use types::{
    BayerPattern, DataType, FileFormat, HeaderKeyword, ImageHeader, ImageMetadata, PixelData,
    ProcessedImage,
};
