// dd-ocr-rs Tesseract bridge — a tiny C ABI shim over the Tesseract C++ API
// (libtesseract) and Leptonica (liblept), the local open-source OCR stack.
//
// Image bytes cross the FFI boundary as a raw buffer; Leptonica decodes them
// (pixReadMem auto-detects PNG/JPEG/TIFF/BMP/...), Tesseract recognises text,
// and the UTF-8 result plus any error string are malloc'd here and freed back
// through dd_tess_free_str. Mean word confidence (0..100) is returned through
// out_conf.
//
// Compiled + linked only when the `tesseract-bridge` Cargo feature is on
// (see build.rs); otherwise the Rust side returns TesseractError::Disabled.

#include <tesseract/baseapi.h>
// Leptonica's pkg-config Cflags point at the leptonica include dir, but the
// header lives under <leptonica/...> on Debian (the Docker target) and at the
// top level on Homebrew. Accept either layout.
#if __has_include(<leptonica/allheaders.h>)
#include <leptonica/allheaders.h>
#else
#include <allheaders.h>
#endif

#include <cstdlib>
#include <cstring>
#include <string>

extern "C" {

// Duplicate a std::string into a malloc'd, NUL-terminated C string the Rust
// side takes ownership of (freed via dd_tess_free_str).
static char *dd_dup_cstr(const std::string &s) {
    char *out = static_cast<char *>(std::malloc(s.size() + 1));
    if (out != nullptr) {
        std::memcpy(out, s.c_str(), s.size() + 1);
    }
    return out;
}

// Recognise text in the image `data`/`len` using `lang` (e.g. "eng",
// "eng+deu"). `psm` selects the Tesseract page-segmentation mode (0..13; any
// other value leaves the engine default). On success returns malloc'd UTF-8
// text and writes the mean word confidence to *out_conf. On failure returns
// nullptr and sets *out_err to a malloc'd message.
char *dd_tess_ocr(const unsigned char *data, size_t len, const char *lang,
                  int psm, int *out_conf, char **out_err) {
    if (out_err != nullptr) {
        *out_err = nullptr;
    }
    if (out_conf != nullptr) {
        *out_conf = -1;
    }

    Pix *pix = pixReadMem(data, len);
    if (pix == nullptr) {
        if (out_err != nullptr) {
            *out_err = dd_dup_cstr("leptonica could not decode the image bytes");
        }
        return nullptr;
    }

    tesseract::TessBaseAPI api;
    const char *language = (lang != nullptr && lang[0] != '\0') ? lang : "eng";
    // tessdata location comes from TESSDATA_PREFIX in the environment.
    if (api.Init(nullptr, language) != 0) {
        pixDestroy(&pix);
        if (out_err != nullptr) {
            *out_err = dd_dup_cstr(std::string("tesseract Init failed for language(s): ") + language);
        }
        return nullptr;
    }
    if (psm >= 0 && psm <= 13) {
        api.SetPageSegMode(static_cast<tesseract::PageSegMode>(psm));
    }
    api.SetImage(pix);

    char *text = api.GetUTF8Text();
    int conf = api.MeanTextConf();
    if (out_conf != nullptr) {
        *out_conf = conf;
    }

    std::string result = (text != nullptr) ? std::string(text) : std::string();
    if (text != nullptr) {
        delete[] text; // GetUTF8Text allocates with new[].
    }
    api.End();
    pixDestroy(&pix);

    return dd_dup_cstr(result);
}

// The linked Tesseract library version, e.g. "5.3.4".
char *dd_tess_version() {
    return dd_dup_cstr(std::string(tesseract::TessBaseAPI::Version()));
}

void dd_tess_free_str(char *ptr) { std::free(ptr); }

} // extern "C"
