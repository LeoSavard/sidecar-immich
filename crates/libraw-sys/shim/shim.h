/* SPDX-License-Identifier: Apache-2.0
 * Flat C interface over the parts of LibRaw that Sidecar uses. Keeps Rust free of LibRaw's struct layout. */
#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct sc_info {
  int32_t width, height;         /* visible sensor area, before orientation */
  int32_t flip;                  /* LibRaw flip: 0, 3 (180°), 5 (90° CCW), 6 (90° CW) */
  int32_t colors;
  uint32_t filters;              /* 9 = X-Trans, 0 = no CFA */
  int32_t is_dng;
  float black, maximum;
  float cam_mul[4], pre_mul[4];
  float cam_xyz[4][3];           /* XYZ -> camera, from LibRaw's colour data */
  uint16_t dng_illuminant[2];
  float dng_colormatrix[2][4][3];
  float iso, shutter, aperture, focal_len;
  int64_t timestamp;
  char make[64], model[64], lens[128];
} sc_info;

typedef struct sc_image {
  int32_t width, height;         /* after orientation */
  uint16_t *data;                /* RGB, 16 bits, row-major */
  void *handle;
} sc_image;

void *sc_new(void);
void sc_free(void *raw);
int sc_open(void *raw, const char *path);
int sc_unpack(void *raw);
void sc_get_info(void *raw, sc_info *info);
/* Demosaic to linear camera RGB, no white balance, sensor saturation at 65535, orientation applied.
 * half_size = 1 skips interpolation (2×2 binning); quality selects LibRaw's demosaic (11 = DHT). */
int sc_develop_linear(void *raw, int half_size, int quality, sc_image *out);
void sc_image_free(sc_image *img);

typedef struct sc_mosaic {
  int32_t width, height;         /* visible area, before orientation */
  uint16_t *data;                /* one sample per pixel, row-major; free with sc_mosaic_free */
  float black[4];                /* per CFA colour index 0..3 (R, G, B, G2), raw units */
  float white;                   /* raw saturation level */
  uint8_t pattern[6][6];         /* colour index of visible (row % n, col % n): n = 6 for X-Trans, 2 for Bayer */
  int32_t period;                /* 2 or 6 */
} sc_mosaic;

/* Copy the visible mosaic. Fails for images without a single-channel CFA (sRAW, linear DNG, Foveon). */
int sc_get_mosaic(void *raw, sc_mosaic *out);
void sc_mosaic_free(sc_mosaic *m);
const char *sc_strerror(int code);

#ifdef __cplusplus
}
#endif
