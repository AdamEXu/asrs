use asrs_js::math;

use super::Vector3;
use super::math_utils::clamp;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spherical {
    pub radius: f64,
    pub phi: f64,
    pub theta: f64,
}

impl Default for Spherical {
    fn default() -> Self {
        Spherical::new(1.0, 0.0, 0.0)
    }
}

impl Spherical {
    pub const fn new(radius: f64, phi: f64, theta: f64) -> Self {
        Spherical { radius, phi, theta }
    }

    pub fn make_safe(&mut self) {
        const EPS: f64 = 0.000001;
        self.phi = math::max2(EPS, math::min2(math::PI - EPS, self.phi));
    }

    pub fn from_vector3(v: Vector3) -> Self {
        Spherical::from_cartesian_coords(v.x, v.y, v.z)
    }

    pub fn from_cartesian_coords(x: f64, y: f64, z: f64) -> Self {
        let radius = math::sqrt(x * x + y * y + z * z);
        if radius == 0.0 {
            Spherical::new(radius, 0.0, 0.0)
        } else {
            Spherical::new(
                radius,
                math::acos(clamp(y / radius, -1.0, 1.0)),
                math::atan2(x, z),
            )
        }
    }

    pub fn to_vector3(self) -> Vector3 {
        Vector3::from_spherical_coords(self.radius, self.phi, self.theta)
    }
}
