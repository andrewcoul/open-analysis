//! Orthographic orbit camera. World coordinates are metres; screen
//! coordinates are pixels with y down. Pure arithmetic, no GPUI.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpAxis {
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewPreset {
    ThreeD,
    Plan,
    /// Looking along the horizontal axis that is not X, so X is across.
    ElevationX,
    /// Looking along X.
    ElevationY,
}

#[derive(Clone, Debug)]
pub struct Camera {
    /// Rotation about the vertical axis, radians.
    pub yaw: f64,
    /// Tilt down toward a plan view, radians. Positive looks from above.
    pub pitch: f64,
    /// World point at the centre of the viewport.
    pub target: [f64; 3],
    /// Pixels per metre.
    pub scale: f64,
    pub up: UpAxis,
}

impl Default for Camera {
    fn default() -> Self {
        let mut camera = Self {
            yaw: 0.0,
            pitch: 0.0,
            target: [0.0; 3],
            scale: 40.0,
            up: UpAxis::Z,
        };
        camera.set_preset(ViewPreset::ThreeD);
        camera
    }
}

impl Camera {
    /// World to the standard frame: x across, y up, z toward the viewer.
    fn standardize(&self, p: [f64; 3]) -> [f64; 3] {
        match self.up {
            UpAxis::Y => p,
            UpAxis::Z => [p[0], p[2], -p[1]],
        }
    }
    fn unstandardize(&self, s: [f64; 3]) -> [f64; 3] {
        match self.up {
            UpAxis::Y => s,
            UpAxis::Z => [s[0], -s[2], s[1]],
        }
    }
    /// Rows of the view rotation, in the standard frame.
    fn rows(&self) -> [[f64; 3]; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [
            [cy, 0.0, sy],
            [sp * sy, cp, -sp * cy],
            [-cp * sy, sp, cp * cy],
        ]
    }
    fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }
    /// View-space coordinates relative to the target: (across, up, depth) in metres.
    pub fn view(&self, p: [f64; 3]) -> [f64; 3] {
        let t = self.standardize(self.target);
        let s = self.standardize(p);
        let d = [s[0] - t[0], s[1] - t[1], s[2] - t[2]];
        let r = self.rows();
        [Self::dot(r[0], d), Self::dot(r[1], d), Self::dot(r[2], d)]
    }
    /// Screen position given the viewport centre, plus depth (larger is nearer).
    pub fn project(&self, p: [f64; 3], centre: (f64, f64)) -> (f64, f64, f64) {
        let v = self.view(p);
        (
            centre.0 + v[0] * self.scale,
            centre.1 - v[1] * self.scale,
            v[2],
        )
    }
    /// Screen-space direction of a world direction, unit length in pixels per metre.
    pub fn project_direction(&self, d: [f64; 3]) -> (f64, f64) {
        let s = self.standardize(d);
        let r = self.rows();
        (Self::dot(r[0], s), -Self::dot(r[1], s))
    }
    /// World directions that appear as screen right, screen up, and toward the viewer.
    fn axes(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let r = self.rows();
        (
            self.unstandardize(r[0]),
            self.unstandardize(r[1]),
            self.unstandardize(r[2]),
        )
    }

    pub fn orbit(&mut self, dx: f64, dy: f64) {
        self.yaw += dx * 0.01;
        self.pitch = (self.pitch + dy * 0.01)
            .clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
    }
    /// Moves the target so the scene follows a drag of (dx, dy) pixels.
    pub fn pan(&mut self, dx: f64, dy: f64) {
        let (right, up, _) = self.axes();
        for i in 0..3 {
            self.target[i] += -right[i] * dx / self.scale + up[i] * dy / self.scale;
        }
    }
    /// Scales about the point `offset` pixels from the viewport centre.
    pub fn zoom(&mut self, factor: f64, offset: (f64, f64)) {
        let old = self.scale;
        let new = (old * factor).clamp(1e-3, 1e6);
        let (right, up, _) = self.axes();
        let k = 1.0 / old - 1.0 / new;
        for i in 0..3 {
            self.target[i] += right[i] * offset.0 * k - up[i] * offset.1 * k;
        }
        self.scale = new;
    }
    /// Frames the points in a viewport of the given size, keeping the rotation.
    pub fn fit(&mut self, points: impl IntoIterator<Item = [f64; 3]>, width: f64, height: f64) {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let r = self.rows();
        let mut any = false;
        for p in points {
            any = true;
            let s = self.standardize(p);
            for i in 0..3 {
                let v = Self::dot(r[i], s);
                min[i] = min[i].min(v);
                max[i] = max[i].max(v);
            }
        }
        if !any {
            self.target = [0.0; 3];
            self.scale = 40.0;
            return;
        }
        let centre = [
            (min[0] + max[0]) / 2.0,
            (min[1] + max[1]) / 2.0,
            (min[2] + max[2]) / 2.0,
        ];
        // The rotation is orthonormal, so its transpose maps view space back.
        let mut s = [0.0; 3];
        for i in 0..3 {
            s[i] = r[0][i] * centre[0] + r[1][i] * centre[1] + r[2][i] * centre[2];
        }
        self.target = self.unstandardize(s);
        let extent_x = (max[0] - min[0]).max(1e-6);
        let extent_y = (max[1] - min[1]).max(1e-6);
        let scale = (width / extent_x).min(height / extent_y) * 0.8;
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale.clamp(1e-3, 1e6)
        } else {
            40.0
        };
    }
    /// World point under a screen position. With `plane` as `(axis, value)`
    /// the point is where the view ray meets that plane; when the plane is
    /// edge-on, or no plane is given, the point lies in the view plane through
    /// the target.
    pub fn unproject(
        &self,
        screen: (f64, f64),
        centre: (f64, f64),
        plane: Option<(usize, f64)>,
    ) -> [f64; 3] {
        let (right, up, toward) = self.axes();
        let dx = (screen.0 - centre.0) / self.scale;
        let dy = (screen.1 - centre.1) / self.scale;
        let mut p = [0.0; 3];
        for i in 0..3 {
            p[i] = self.target[i] + right[i] * dx - up[i] * dy;
        }
        if let Some((axis, value)) = plane
            && toward[axis].abs() > 1e-6
        {
            let t = (value - p[axis]) / toward[axis];
            for i in 0..3 {
                p[i] += toward[i] * t;
            }
            p[axis] = value;
        }
        p
    }
    pub fn set_preset(&mut self, preset: ViewPreset) {
        let (yaw, pitch) = match preset {
            ViewPreset::ThreeD => (-35f64.to_radians(), 25f64.to_radians()),
            ViewPreset::Plan => (0.0, std::f64::consts::FRAC_PI_2),
            ViewPreset::ElevationX => (0.0, 0.0),
            ViewPreset::ElevationY => (-std::f64::consts::FRAC_PI_2, 0.0),
        };
        self.yaw = yaw;
        self.pitch = pitch;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn plan_view_shows_x_right_and_y_up_with_z_up() {
        let mut camera = Camera::default();
        camera.set_preset(ViewPreset::Plan);
        camera.scale = 1.0;
        let (x, y, _) = camera.project([1.0, 0.0, 0.0], (0.0, 0.0));
        assert!(close(x, 1.0) && close(y, 0.0));
        let (x, y, _) = camera.project([0.0, 1.0, 0.0], (0.0, 0.0));
        assert!(
            close(x, 0.0) && close(y, -1.0),
            "y should point up the screen: {x} {y}"
        );
    }

    #[test]
    fn elevation_shows_z_up() {
        let mut camera = Camera::default();
        camera.set_preset(ViewPreset::ElevationX);
        camera.scale = 2.0;
        let (x, y, _) = camera.project([0.0, 0.0, 1.0], (10.0, 10.0));
        assert!(close(x, 10.0) && close(y, 8.0));
    }

    #[test]
    fn pan_keeps_dragged_point_under_cursor() {
        let mut camera = Camera::default();
        let before = camera.project([3.0, 1.0, 2.0], (0.0, 0.0));
        camera.pan(15.0, -7.0);
        let after = camera.project([3.0, 1.0, 2.0], (0.0, 0.0));
        assert!(close(after.0 - before.0, 15.0));
        assert!(close(after.1 - before.1, -7.0));
    }

    #[test]
    fn zoom_keeps_point_under_cursor_fixed() {
        let mut camera = Camera::default();
        let p = [3.0, 1.0, 2.0];
        let (x, y, _) = camera.project(p, (0.0, 0.0));
        camera.zoom(1.5, (x, y));
        let (x2, y2, _) = camera.project(p, (0.0, 0.0));
        assert!(close(x, x2) && close(y, y2));
    }

    #[test]
    fn unproject_returns_to_the_ground_plane() {
        let camera = Camera::default();
        let p = [3.0, 4.0, 0.0];
        let (x, y, _) = camera.project(p, (400.0, 300.0));
        let back = camera.unproject((x, y), (400.0, 300.0), Some((2, 0.0)));
        for i in 0..3 {
            assert!(close(back[i], p[i]), "{back:?}");
        }
    }

    #[test]
    fn unproject_falls_back_to_the_view_plane_when_edge_on() {
        let mut camera = Camera::default();
        camera.set_preset(ViewPreset::ElevationX);
        camera.target = [1.0, 2.0, 3.0];
        let (x, y, _) = camera.project([5.0, 2.0, 7.0], (0.0, 0.0));
        let back = camera.unproject((x, y), (0.0, 0.0), Some((2, 0.0)));
        assert!(
            close(back[0], 5.0) && close(back[1], 2.0) && close(back[2], 7.0),
            "{back:?}"
        );
    }

    #[test]
    fn fit_centres_and_scales() {
        let mut camera = Camera::default();
        camera.set_preset(ViewPreset::Plan);
        let points = [[0.0, 0.0, 0.0], [10.0, 4.0, 0.0]];
        camera.fit(points, 800.0, 600.0);
        let (x, y, _) = camera.project([5.0, 2.0, 0.0], (400.0, 300.0));
        assert!(close(x, 400.0) && close(y, 300.0));
        assert!(close(camera.scale, 64.0));
    }
}
