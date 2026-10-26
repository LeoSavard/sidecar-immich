/* SPDX-License-Identifier: Apache-2.0
 * The browser's entry into LibRaw, built with Emscripten by mobile/libraw/build.sh: the server's own shim
 * (crates/libraw-sys/shim) over bytes in memory, with what it reads handed back as JSON rather than C structs, so
 * the TypeScript side mirrors no struct layout. One decode at a time per module instance (a Web Worker has one).
 *
 * The JSON names are the ones `sidecar_color::camera::LibrawColor` reads (camelCase), so the colour half of the
 * info goes to the colour wasm as it is. */
#include "shim.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

namespace {
void *raw = nullptr;
sc_mosaic mosaic = {};
std::string text;

void num(std::string &o, const char *key, double v) {
  char b[64];
  std::snprintf(b, sizeof b, "\"%s\":%.9g,", key, v);
  o += b;
}

template <size_t N> void nums(std::string &o, const char *key, const float (&v)[N]) {
  o += "\"";
  o += key;
  o += "\":[";
  for (size_t i = 0; i < N; i++) {
    char b[32];
    std::snprintf(b, sizeof b, "%s%.9g", i ? "," : "", v[i]);
    o += b;
  }
  o += "],";
}

void str(std::string &o, const char *key, const char *v) {
  o += "\"";
  o += key;
  o += "\":\"";
  for (const unsigned char *c = (const unsigned char *)v; *c; c++) {
    if (*c == '"' || *c == '\\') o += '\\', o += (char)*c;
    else if (*c >= 0x20 && *c < 0x7f) o += (char)*c;
    // Anything else (control bytes, a maker's non-ASCII) is left out rather than risk invalid JSON.
  }
  o += "\",";
}

const char *done(std::string &o) {
  if (!o.empty() && o.back() == ',') o.pop_back();
  o += "}";
  return o.c_str();
}
} // namespace

extern "C" {

/* Open and unpack `size` bytes at `data`, which must stay alive until scw_close. 0 or a LibRaw error code. */
int scw_open(const void *data, size_t size) {
  if (raw) sc_free(raw);
  raw = sc_new();
  int r = sc_open_buffer(raw, data, size);
  return r ? r : sc_unpack(raw);
}

const char *scw_strerror(int code) { return sc_strerror(code); }

/* What `sidecar_raw::RawInfo` is built from: size, orientation, the CFA kind, exposure, and the colour data. */
const char *scw_info(void) {
  sc_info i;
  sc_get_info(raw, &i);
  text = "{";
  num(text, "width", i.width);
  num(text, "height", i.height);
  num(text, "flip", i.flip);
  num(text, "colors", i.colors);
  num(text, "filters", i.filters);
  num(text, "iso", i.iso);
  num(text, "shutter", i.shutter);
  num(text, "aperture", i.aperture);
  num(text, "focalLen", i.focal_len);
  num(text, "timestamp", (double)i.timestamp);
  str(text, "make", i.make);
  str(text, "model", i.model);
  str(text, "lens", i.lens);
  text += "\"color\":{";
  nums(text, "camMul", i.cam_mul);
  nums(text, "preMul", i.pre_mul);
  text += "\"camXyz\":[";
  for (int r = 0; r < 4; r++) {
    char b[96];
    std::snprintf(b, sizeof b, "%s[%.9g,%.9g,%.9g]", r ? "," : "", i.cam_xyz[r][0], i.cam_xyz[r][1], i.cam_xyz[r][2]);
    text += b;
  }
  text += "],\"dngIlluminant\":[";
  text += std::to_string(i.dng_illuminant[0]) + "," + std::to_string(i.dng_illuminant[1]) + "],\"dngColormatrix\":[";
  for (int n = 0; n < 2; n++) {
    text += n ? ",[" : "[";
    for (int r = 0; r < 4; r++) {
      char b[96];
      std::snprintf(b, sizeof b, "%s[%.9g,%.9g,%.9g]", r ? "," : "", i.dng_colormatrix[n][r][0], i.dng_colormatrix[n][r][1],
                    i.dng_colormatrix[n][r][2]);
      text += b;
    }
    text += "]";
  }
  text += "]},";
  return done(text);
}

/* Copy the visible mosaic and describe it; its samples are at scw_mosaic_data() until scw_close. An empty
 * string when the file has no single-channel CFA (sRAW, linear DNG, Foveon): the server decodes those. */
const char *scw_mosaic(void) {
  sc_mosaic_free(&mosaic);
  if (sc_get_mosaic(raw, &mosaic) != 0) return "";
  text = "{";
  num(text, "width", mosaic.width);
  num(text, "height", mosaic.height);
  nums(text, "black", mosaic.black);
  num(text, "white", mosaic.white);
  num(text, "period", mosaic.period);
  text += "\"pattern\":[";
  for (int y = 0; y < 6; y++)
    for (int x = 0; x < 6; x++) text += std::to_string(mosaic.pattern[y][x]) + (y == 5 && x == 5 ? "" : ",");
  text += "],";
  return done(text);
}

const uint16_t *scw_mosaic_data(void) { return mosaic.data; }

void scw_close(void) {
  sc_mosaic_free(&mosaic);
  if (raw) sc_free(raw);
  raw = nullptr;
}
}
