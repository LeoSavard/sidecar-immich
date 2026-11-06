// SPDX-License-Identifier: Apache-2.0
//! The small dense linear algebra the solvers need: 3 × 3 rotations, a symmetric eigensolver, and Gaussian
//! elimination. Nothing here is large enough to want a library.

// Matrix code reads best with its subscripts.
#![allow(clippy::needless_range_loop)]

pub type V3 = [f64; 3];
pub type M3 = [[f64; 3]; 3];

pub const I3: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

pub fn mulv(m: &M3, v: V3) -> V3 {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

pub fn transpose(m: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]))
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

pub fn normalize(a: V3) -> V3 {
    let n = norm(a);
    if n > 0.0 { a.map(|v| v / n) } else { a }
}

/// The rotation by angle |w| about axis w (Rodrigues).
pub fn rodrigues(w: V3) -> M3 {
    let t = norm(w);
    if t < 1e-15 {
        return [[1.0, -w[2], w[1]], [w[2], 1.0, -w[0]], [-w[1], w[0], 1.0]];
    }
    let k = w.map(|v| v / t);
    let (s, c) = t.sin_cos();
    let kx: M3 = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]];
    let kk = mul(&kx, &kx);
    std::array::from_fn(|i| std::array::from_fn(|j| I3[i][j] + s * kx[i][j] + (1.0 - c) * kk[i][j]))
}

/// The rotation of a unit quaternion (w, x, y, z).
pub fn from_quaternion(q: [f64; 4]) -> M3 {
    let n = (q.iter().map(|v| v * v).sum::<f64>()).sqrt();
    let [w, x, y, z] = q.map(|v| v / n);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// The rotation `R` minimising Σ |R·wₖ − vₖ|², by Horn's closed form ("Closed-form solution of absolute
/// orientation using unit quaternions", JOSA A 1987): the quaternion is the eigenvector of the largest
/// eigenvalue of a symmetric 4 × 4 matrix built from the correlation Σ wₖ vₖᵀ.
pub fn absolute_orientation(w: &[V3], v: &[V3]) -> M3 {
    let mut s = [[0f64; 3]; 3];
    for (a, b) in w.iter().zip(v) {
        for i in 0..3 {
            for j in 0..3 {
                s[i][j] += a[i] * b[j];
            }
        }
    }
    let [[xx, xy, xz], [yx, yy, yz], [zx, zy, zz]] = s;
    let n = vec![
        vec![xx + yy + zz, yz - zy, zx - xz, xy - yx],
        vec![yz - zy, xx - yy - zz, xy + yx, zx + xz],
        vec![zx - xz, xy + yx, -xx + yy - zz, yz + zy],
        vec![xy - yx, zx + xz, yz + zy, -xx - yy + zz],
    ];
    let (vals, vecs) = symmetric_eigen(n);
    let k = (0..4).max_by(|&a, &b| vals[a].total_cmp(&vals[b])).unwrap();
    from_quaternion([vecs[0][k], vecs[1][k], vecs[2][k], vecs[3][k]])
}

/// Eigenvalues and eigenvectors (as columns) of a symmetric matrix, by cyclic Jacobi rotations.
pub fn symmetric_eigen(mut a: Vec<Vec<f64>>) -> (Vec<f64>, Vec<Vec<f64>>) {
    let n = a.len();
    let mut v: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for _ in 0..100 {
        let off: f64 = (0..n)
            .flat_map(|i| (0..n).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| a[i][j] * a[i][j])
            .sum();
        if off < 1e-30 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (akp, akq) = (a[k][p], a[k][q]);
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let (apk, aqk) = (a[p][k], a[q][k]);
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for row in v.iter_mut() {
                    let (vkp, vkq) = (row[p], row[q]);
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    ((0..n).map(|i| a[i][i]).collect(), v)
}

/// Solve `a · x = b` by Gaussian elimination with partial pivoting; `None` when singular.
pub fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let p = (c..n).max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs()))?;
        if a[p][c].abs() < 1e-300 {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in c + 1..n {
            let f = a[r][c] / a[c][c];
            if f != 0.0 {
                for k in c..n {
                    a[r][k] -= f * a[c][k];
                }
                b[r] -= f * b[c];
            }
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|k| a[r][k] * x[k]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horn_recovers_a_rotation() {
        let r = rodrigues([0.3, -0.7, 0.2]);
        let w: Vec<V3> = (0..20)
            .map(|k| {
                normalize([
                    (k as f64).sin(),
                    (k as f64 * 1.7).cos(),
                    1.0 + k as f64 * 0.1,
                ])
            })
            .collect();
        let v: Vec<V3> = w.iter().map(|&p| mulv(&r, p)).collect();
        let got = absolute_orientation(&w, &v);
        for i in 0..3 {
            for j in 0..3 {
                assert!((got[i][j] - r[i][j]).abs() < 1e-9, "{got:?} against {r:?}");
            }
        }
    }

    #[test]
    fn eigen_and_solve() {
        let a = vec![
            vec![4.0, 1.0, 0.5],
            vec![1.0, 3.0, 0.2],
            vec![0.5, 0.2, 1.0],
        ];
        let (vals, vecs) = symmetric_eigen(a.clone());
        for k in 0..3 {
            for i in 0..3 {
                let av: f64 = (0..3).map(|j| a[i][j] * vecs[j][k]).sum();
                assert!((av - vals[k] * vecs[i][k]).abs() < 1e-10);
            }
        }
        let x = solve(a.clone(), vec![1.0, 2.0, 3.0]).unwrap();
        for i in 0..3 {
            let ax: f64 = (0..3).map(|j| a[i][j] * x[j]).sum();
            assert!((ax - [1.0, 2.0, 3.0][i]).abs() < 1e-12);
        }
    }
}
