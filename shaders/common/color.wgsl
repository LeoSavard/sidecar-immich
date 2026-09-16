// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Rec.2020 luminance and OKLab (Björn Ottosson) on linear Rec.2020 (D65).
// Matrices are written as rows and applied with dot products, so there is no column-order ambiguity.

const LUMA_2020 = vec3f(0.2627002120, 0.6779980715, 0.0593017165);

const R2020_TO_LMS_0 = vec3f(0.6166884418, 0.3601590705, 0.0230432935);
const R2020_TO_LMS_1 = vec3f(0.2651401962, 0.6358564847, 0.0990302336);
const R2020_TO_LMS_2 = vec3f(0.1001506451, 0.2040043234, 0.6963246774);

const LMS_TO_R2020_0 = vec3f(2.1401404110, -1.2463559505, 0.1064317259);
const LMS_TO_R2020_1 = vec3f(-0.8848324528, 2.1631727207, -0.2783615921);
const LMS_TO_R2020_2 = vec3f(-0.0485790580, -0.4544909080, 1.5023562946);

fn cbrt_signed(x: f32) -> f32 {
  return sign(x) * pow(abs(x), 1.0 / 3.0);
}

fn rec2020_to_oklab(c: vec3f) -> vec3f {
  let l = cbrt_signed(dot(R2020_TO_LMS_0, c));
  let m = cbrt_signed(dot(R2020_TO_LMS_1, c));
  let s = cbrt_signed(dot(R2020_TO_LMS_2, c));
  return vec3f(
    0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
  );
}

fn oklab_to_rec2020(lab: vec3f) -> vec3f {
  let l_ = lab.x + 0.3963377774 * lab.y + 0.2158037573 * lab.z;
  let m_ = lab.x - 0.1055613458 * lab.y - 0.0638541728 * lab.z;
  let s_ = lab.x - 0.0894841775 * lab.y - 1.2914855480 * lab.z;
  let lms = vec3f(l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
  return vec3f(dot(LMS_TO_R2020_0, lms), dot(LMS_TO_R2020_1, lms), dot(LMS_TO_R2020_2, lms));
}
