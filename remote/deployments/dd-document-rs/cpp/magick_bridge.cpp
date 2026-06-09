// C ABI shim over the official ImageMagick C++ SDK (Magick++).
//
// Rust cannot call C++ directly, so this thin layer exposes a handful of
// `extern "C"` entry points that the `dd-document-rs` service binds to in
// src/image_ffi.rs. All image bytes cross as raw buffers; ownership of returned
// buffers/strings transfers to the caller, who must release them with
// dd_magick_free_blob / dd_magick_free_str.
//
// Hardening baked in here (defence in depth alongside the shipped policy.xml):
//   * ImageMagick ResourceLimits cap memory/area/disk/time per process, which
//     blunts decompression bombs.
//   * Callers screen output formats; this layer also rejects decoded *input*
//     coders outside a safe raster allowlist (blocks MVG/MSL/SVG/PDF/URL/...),
//     and metadata is stripped by default.

#include <Magick++.h>

#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <set>
#include <sstream>
#include <string>

namespace {

// One-time Magick++ initialization + conservative global resource limits.
// Function-local static initialization is thread-safe under C++11.
bool ensure_initialized() {
  static bool initialized = [] {
    Magick::InitializeMagick(nullptr);
    using Magick::ResourceLimits;
    ResourceLimits::memory(256ULL * 1024 * 1024);  // 256 MiB pixel cache
    ResourceLimits::map(512ULL * 1024 * 1024);     // 512 MiB memory-map
    ResourceLimits::disk(1024ULL * 1024 * 1024);   // 1 GiB disk cache
    ResourceLimits::area(64ULL * 1024 * 1024);     // 64 MP working area
    ResourceLimits::thread(2ULL);
    // Pixel width/height ceilings are enforced via the shipped policy.xml
    // (ResourceLimits::width/height are not portable to ImageMagick 6).
    return true;
  }();
  return initialized;
}

// Raster coders we accept on the *decode* side. Anything else (vector, script,
// document, or pseudo coders such as MVG/MSL/SVG/PDF/PS/URL/HTTPS/EPHEMERAL/
// LABEL/CAPTION/TEXT/MAGICK/...) is rejected after sniffing.
const std::set<std::string>& input_allowlist() {
  static const std::set<std::string> allow = {
      "PNG",  "PNG8", "PNG24", "PNG32", "JPEG", "JPG",  "JPE",  "GIF",
      "BMP",  "BMP2", "BMP3",  "WEBP",  "TIFF", "TIF",  "HEIC", "HEIF",
      "AVIF", "ICO",  "PNM",   "PPM",   "PGM",  "PBM",  "TGA",  "DDS",
  };
  return allow;
}

char* dup_cstr(const std::string& s) {
  char* out = static_cast<char*>(std::malloc(s.size() + 1));
  if (out) {
    std::memcpy(out, s.c_str(), s.size() + 1);
  }
  return out;
}

void set_error(char** err, const std::string& message) {
  if (err) {
    *err = dup_cstr(message);
  }
}

std::string json_escape(const std::string& in) {
  std::ostringstream out;
  for (char c : in) {
    switch (c) {
      case '"': out << "\\\""; break;
      case '\\': out << "\\\\"; break;
      case '\n': out << "\\n"; break;
      case '\r': out << "\\r"; break;
      case '\t': out << "\\t"; break;
      default:
        if (static_cast<unsigned char>(c) < 0x20) {
          out << "\\u" << std::hex << std::uppercase;
          out.width(4);
          out.fill('0');
          out << static_cast<int>(static_cast<unsigned char>(c));
          out << std::dec;
        } else {
          out << c;
        }
    }
  }
  return out.str();
}

std::string upper(std::string s) {
  for (char& c : s) {
    c = static_cast<char>(std::toupper(static_cast<unsigned char>(c)));
  }
  return s;
}

}  // namespace

// Mirror of the Rust #[repr(C)] DdMagickOp. Null pointers / zero values mean
// "leave unchanged".
extern "C" struct DdMagickOp {
  const char* out_format;  // target encoder, e.g. "PNG" (nullable)
  const char* resize;      // ImageMagick geometry, e.g. "200x200>" (nullable)
  const char* crop;        // crop geometry, e.g. "100x100+10+10" (nullable)
  double rotate_degrees;   // 0 = no rotation
  int quality;             // 0 = encoder default, else 1..100
  int strip;               // !=0 strips metadata/profiles
  int grayscale;           // !=0 converts to grayscale
  int auto_orient;         // !=0 applies EXIF orientation then clears the tag
  const char* background;  // flatten background colour (nullable)
};

extern "C" int dd_magick_transform(const uint8_t* in, size_t in_len,
                                   const DdMagickOp* op, uint8_t** out,
                                   size_t* out_len, char** err) {
  if (out) *out = nullptr;
  if (out_len) *out_len = 0;
  if (err) *err = nullptr;
  if (!in || in_len == 0 || !op || !out || !out_len) {
    set_error(err, "invalid arguments to dd_magick_transform");
    return 1;
  }
  ensure_initialized();
  try {
    Magick::Image image;
    Magick::Blob in_blob(in, in_len);
    image.read(in_blob);

    const std::string detected = upper(image.magick());
    if (input_allowlist().find(detected) == input_allowlist().end()) {
      set_error(err, "input coder '" + detected + "' is not permitted");
      return 2;
    }

    // EXIF auto-orient first so subsequent geometry ops act on upright pixels.
    if (op->auto_orient) {
      image.autoOrient();
    }
    if (op->background) {
      // Flatten transparency onto the background colour (== `-alpha remove`).
      image.backgroundColor(Magick::Color(op->background));
      image.alphaChannel(Magick::RemoveAlphaChannel);
    }
    if (op->crop && op->crop[0] != '\0') {
      image.crop(Magick::Geometry(op->crop));
      image.page(Magick::Geometry(0, 0));  // reset virtual canvas offset
    }
    if (op->resize && op->resize[0] != '\0') {
      image.resize(Magick::Geometry(op->resize));
    }
    if (op->rotate_degrees != 0.0) {
      image.rotate(op->rotate_degrees);
    }
    if (op->grayscale) {
      image.type(Magick::GrayscaleType);
    }
    if (op->quality > 0) {
      image.quality(static_cast<size_t>(op->quality));
    }
    if (op->strip) {
      image.strip();
    }
    if (op->out_format && op->out_format[0] != '\0') {
      image.magick(op->out_format);
    }

    Magick::Blob out_blob;
    image.write(&out_blob);

    const size_t len = out_blob.length();
    uint8_t* buf = static_cast<uint8_t*>(std::malloc(len ? len : 1));
    if (!buf) {
      set_error(err, "out-of-memory allocating result buffer");
      return 3;
    }
    std::memcpy(buf, out_blob.data(), len);
    *out = buf;
    *out_len = len;
    return 0;
  } catch (Magick::Exception& e) {
    set_error(err, std::string("magick error: ") + e.what());
    return 4;
  } catch (std::exception& e) {
    set_error(err, std::string("error: ") + e.what());
    return 5;
  } catch (...) {
    set_error(err, "unknown image processing error");
    return 6;
  }
}

extern "C" int dd_magick_identify(const uint8_t* in, size_t in_len,
                                  char** out_json, char** err) {
  if (out_json) *out_json = nullptr;
  if (err) *err = nullptr;
  if (!in || in_len == 0 || !out_json) {
    set_error(err, "invalid arguments to dd_magick_identify");
    return 1;
  }
  ensure_initialized();
  try {
    Magick::Image image;
    Magick::Blob in_blob(in, in_len);
    image.read(in_blob);

    const std::string format = upper(image.magick());
    if (input_allowlist().find(format) == input_allowlist().end()) {
      set_error(err, "input coder '" + format + "' is not permitted");
      return 2;
    }

    // alpha() is the ImageMagick 7 getter; matte() is the v6 equivalent.
#if defined(MagickLibVersion) && MagickLibVersion >= 0x700
    const bool has_alpha = image.alpha();
#else
    const bool has_alpha = image.matte();
#endif

    std::ostringstream out;
    out << "{"
        << "\"format\":\"" << json_escape(image.magick()) << "\","
        << "\"width\":" << image.columns() << ","
        << "\"height\":" << image.rows() << ","
        << "\"depth\":" << image.depth() << ","
        << "\"colorspace\":\"" << json_escape(std::to_string(image.colorSpace())) << "\","
        << "\"hasAlpha\":" << (has_alpha ? "true" : "false") << ","
        << "\"bytes\":" << in_len
        << "}";
    *out_json = dup_cstr(out.str());
    return 0;
  } catch (Magick::Exception& e) {
    set_error(err, std::string("magick error: ") + e.what());
    return 4;
  } catch (std::exception& e) {
    set_error(err, std::string("error: ") + e.what());
    return 5;
  } catch (...) {
    set_error(err, "unknown image identify error");
    return 6;
  }
}

extern "C" char* dd_magick_version() {
  ensure_initialized();
  // MagickLibVersionText is a global macro string literal (e.g. "7.1.2").
  return dup_cstr(std::string(MagickLibVersionText));
}

extern "C" void dd_magick_free_blob(uint8_t* p) { std::free(p); }
extern "C" void dd_magick_free_str(char* p) { std::free(p); }
