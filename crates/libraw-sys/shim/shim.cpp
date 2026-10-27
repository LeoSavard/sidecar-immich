/* SPDX-License-Identifier: Apache-2.0 */
#include "shim.h"
#include "libraw/libraw.h"
#include <cstdlib>
#include <cstring>

void *sc_new(void) { return new LibRaw(); }
void sc_free(void *raw) { delete static_cast<LibRaw *>(raw); }

int sc_open(void *raw, const char *path) {
  LibRaw *r = static_cast<LibRaw *>(raw);
  r->imgdata.rawparams.max_raw_memory_mb = 8192; /* panoramas and medium format */
  return r->open_file(path);
}
int sc_open_buffer(void *raw, const void *data, size_t size) {
  LibRaw *r = static_cast<LibRaw *>(raw);
  r->imgdata.rawparams.max_raw_memory_mb = 8192;
  return r->open_buffer(data, size);
}
int sc_unpack(void *raw) { return static_cast<LibRaw *>(raw)->unpack(); }

void sc_get_info(void *raw, sc_info *i) {
  LibRaw *r = static_cast<LibRaw *>(raw);
  libraw_data_t &d = r->imgdata;
  std::memset(i, 0, sizeof(*i));
  i->width = d.sizes.width;
  i->height = d.sizes.height;
  i->flip = d.sizes.flip;
  i->colors = d.idata.colors;
  i->filters = d.idata.filters;
  i->is_dng = d.idata.dng_version != 0;
  i->black = (float)d.color.black;
  i->maximum = (float)d.color.maximum;
  for (int c = 0; c < 4; c++) {
    i->cam_mul[c] = d.color.cam_mul[c];
    i->pre_mul[c] = d.color.pre_mul[c];
    for (int k = 0; k < 3; k++) i->cam_xyz[c][k] = d.color.cam_xyz[c][k];
  }
  for (int n = 0; n < 2; n++) {
    i->dng_illuminant[n] = d.color.dng_color[n].illuminant;
    for (int c = 0; c < 4; c++)
      for (int k = 0; k < 3; k++) i->dng_colormatrix[n][c][k] = d.color.dng_color[n].colormatrix[c][k];
    for (int c = 0; c < 4; c++)
      for (int k = 0; k < 4; k++) i->dng_calibration[n][c][k] = d.color.dng_color[n].calibration[c][k];
  }
  for (int c = 0; c < 4; c++) i->dng_analogbalance[c] = d.color.dng_levels.analogbalance[c];
  i->iso = d.other.iso_speed;
  i->shutter = d.other.shutter;
  i->aperture = d.other.aperture;
  i->focal_len = d.other.focal_len;
  i->timestamp = (int64_t)d.other.timestamp;
  std::strncpy(i->make, d.idata.make, sizeof(i->make) - 1);
  std::strncpy(i->model, d.idata.model, sizeof(i->model) - 1);
  std::strncpy(i->lens, d.lens.Lens, sizeof(i->lens) - 1);
}

int sc_develop_linear(void *raw, int half_size, int quality, sc_image *out) {
  LibRaw *r = static_cast<LibRaw *>(raw);
  libraw_output_params_t &p = r->imgdata.params;
  p.half_size = half_size;
  p.user_qual = quality;
  p.use_camera_wb = 0;
  p.use_auto_wb = 0;
  for (int c = 0; c < 4; c++) p.user_mul[c] = 1.0f;
  p.no_auto_bright = 1;
  p.output_color = 0;     /* raw camera colour */
  p.output_bps = 16;
  p.gamm[0] = 1.0;
  p.gamm[1] = 1.0;
  p.highlight = 0;        /* clip at sensor saturation */
  int ret = r->dcraw_process();
  if (ret != LIBRAW_SUCCESS) return ret;
  int err = 0;
  libraw_processed_image_t *img = r->dcraw_make_mem_image(&err);
  if (!img) return err ? err : LIBRAW_UNSPECIFIED_ERROR;
  if (img->type != LIBRAW_IMAGE_BITMAP || img->colors != 3 || img->bits != 16) {
    LibRaw::dcraw_clear_mem(img);
    return LIBRAW_UNSPECIFIED_ERROR;
  }
  out->width = img->width;
  out->height = img->height;
  out->data = (uint16_t *)img->data;
  out->handle = img;
  return LIBRAW_SUCCESS;
}

void sc_image_free(sc_image *img) {
  if (img && img->handle) LibRaw::dcraw_clear_mem(static_cast<libraw_processed_image_t *>(img->handle));
  if (img) img->handle = nullptr;
}

const char *sc_strerror(int code) { return libraw_strerror(code); }

int sc_get_mosaic(void *raw, sc_mosaic *out) {
  LibRaw *r = static_cast<LibRaw *>(raw);
  libraw_data_t &d = r->imgdata;
  if (!d.rawdata.raw_image || !d.idata.filters) return LIBRAW_REQUEST_FOR_NONEXISTENT_IMAGE;
  const int w = d.sizes.width, h = d.sizes.height;
  const int top = d.sizes.top_margin, left = d.sizes.left_margin;
  /* Row stride in samples. LibRaw's own raw2image reads raw_pitch, which a decoder may set wider than
     raw_width (RawSpeed, some DNG paths); raw_width is the fallback for one that leaves it unset. */
  const size_t pitch = d.sizes.raw_pitch ? d.sizes.raw_pitch / sizeof(uint16_t) : (size_t)d.sizes.raw_width;
  if (w <= 0 || h <= 0 || (size_t)(left + w) > pitch) return LIBRAW_REQUEST_FOR_NONEXISTENT_IMAGE;
  uint16_t *buf = static_cast<uint16_t *>(malloc((size_t)w * h * sizeof(uint16_t)));
  if (!buf) return LIBRAW_UNSUFFICIENT_MEMORY;
  for (int y = 0; y < h; y++) std::memcpy(buf + (size_t)y * w, d.rawdata.raw_image + (size_t)(y + top) * pitch + left, w * sizeof(uint16_t));
  out->width = w;
  out->height = h;
  out->data = buf;
  float pattern_mean = 0;
  unsigned pr = d.color.cblack[4], pc = d.color.cblack[5];
  if (pr && pc && pr * pc < LIBRAW_CBLACK_SIZE - 6) {
    for (unsigned i = 0; i < pr * pc; i++) pattern_mean += d.color.cblack[6 + i];
    pattern_mean /= (float)(pr * pc);
  }
  for (int c = 0; c < 4; c++) out->black[c] = (float)d.color.black + (float)d.color.cblack[c] + pattern_mean;
  out->white = (float)d.color.maximum;
  out->period = d.idata.filters == 9 ? 6 : 2;
  for (int y = 0; y < 6; y++)
    for (int x = 0; x < 6; x++) out->pattern[y][x] = (uint8_t)r->COLOR(y, x);
  return LIBRAW_SUCCESS;
}

void sc_mosaic_free(sc_mosaic *m) {
  if (m && m->data) free(m->data);
  if (m) m->data = nullptr;
}
