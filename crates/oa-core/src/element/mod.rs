pub(crate) mod frame;
pub(crate) mod shell;

use nalgebra::{Matrix3, SMatrix, Vector3};

pub(crate) fn rotation(x: Vector3<f64>, y: Vector3<f64>) -> Matrix3<f64> {
    let z = x.cross(&y);
    Matrix3::from_rows(&[x.transpose(), y.transpose(), z.transpose()])
}

pub(crate) fn block_rotation<const N: usize>(r: &Matrix3<f64>) -> SMatrix<f64, N, N> {
    let mut t = SMatrix::<f64, N, N>::zeros();
    for b in 0..N / 3 {
        for i in 0..3 {
            for j in 0..3 {
                t[(3 * b + i, 3 * b + j)] = r[(i, j)];
            }
        }
    }
    t
}

pub(crate) const GAUSS3: [(f64, f64); 3] = [
    (-0.7745966692414834, 5.0 / 9.0),
    (0.0, 8.0 / 9.0),
    (0.7745966692414834, 5.0 / 9.0),
];
