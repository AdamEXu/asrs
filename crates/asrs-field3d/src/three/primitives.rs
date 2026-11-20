//! three's primitive geometry generators. Segment counts are JS numbers, so the loops count in
//! `f64` exactly like the originals, including for fractional, zero or NaN arguments.

use asrs_js::math;

use super::{BufferAttribute, BufferGeometry, Vector3};

fn finish(
    mut geometry: BufferGeometry,
    indices: &[f64],
    vertices: &[f64],
    normals: &[f64],
    uvs: &[f64],
) -> BufferGeometry {
    geometry.set_index(indices);
    geometry.set_attribute("position", BufferAttribute::float32(vertices, 3));
    geometry.set_attribute("normal", BufferAttribute::float32(normals, 3));
    geometry.set_attribute("uv", BufferAttribute::float32(uvs, 2));
    geometry
}

/// `new BoxGeometry(width, height, depth, widthSegments, heightSegments, depthSegments)`.
pub fn box_geometry(
    width: f64,
    height: f64,
    depth: f64,
    width_segments: f64,
    height_segments: f64,
    depth_segments: f64,
) -> BufferGeometry {
    let width_segments = math::floor(width_segments);
    let height_segments = math::floor(height_segments);
    let depth_segments = math::floor(depth_segments);
    let mut geometry = BufferGeometry::new();
    let mut indices = Vec::new();
    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut number_of_vertices = 0.0;
    let mut group_start = 0.0;

    let mut build_plane = |geometry: &mut BufferGeometry,
                           (u, v, w): (usize, usize, usize),
                           udir: f64,
                           vdir: f64,
                           width: f64,
                           height: f64,
                           depth: f64,
                           grid_x: f64,
                           grid_y: f64,
                           material_index: usize| {
        let segment_width = width / grid_x;
        let segment_height = height / grid_y;
        let width_half = width / 2.0;
        let height_half = height / 2.0;
        let depth_half = depth / 2.0;
        let grid_x1 = grid_x + 1.0;
        let grid_y1 = grid_y + 1.0;
        let mut vertex_counter = 0.0;
        let mut group_count = 0.0;
        let mut vector = [0.0; 3];
        let mut iy = 0.0;
        while iy < grid_y1 {
            let y = iy * segment_height - height_half;
            let mut ix = 0.0;
            while ix < grid_x1 {
                let x = ix * segment_width - width_half;
                vector[u] = x * udir;
                vector[v] = y * vdir;
                vector[w] = depth_half;
                vertices.extend_from_slice(&vector);
                vector[u] = 0.0;
                vector[v] = 0.0;
                vector[w] = if depth > 0.0 { 1.0 } else { -1.0 };
                normals.extend_from_slice(&vector);
                uvs.push(ix / grid_x);
                uvs.push(1.0 - (iy / grid_y));
                vertex_counter += 1.0;
                ix += 1.0;
            }
            iy += 1.0;
        }
        let mut iy = 0.0;
        while iy < grid_y {
            let mut ix = 0.0;
            while ix < grid_x {
                let a = number_of_vertices + ix + grid_x1 * iy;
                let b = number_of_vertices + ix + grid_x1 * (iy + 1.0);
                let c = number_of_vertices + (ix + 1.0) + grid_x1 * (iy + 1.0);
                let d = number_of_vertices + (ix + 1.0) + grid_x1 * iy;
                indices.extend_from_slice(&[a, b, d, b, c, d]);
                group_count += 6.0;
                ix += 1.0;
            }
            iy += 1.0;
        }
        geometry.add_group(group_start as usize, group_count as usize, material_index);
        group_start += group_count;
        number_of_vertices += vertex_counter;
    };

    let (x, y, z) = (0, 1, 2);
    let g = &mut geometry;
    build_plane(
        g,
        (z, y, x),
        -1.0,
        -1.0,
        depth,
        height,
        width,
        depth_segments,
        height_segments,
        0,
    );
    build_plane(
        g,
        (z, y, x),
        1.0,
        -1.0,
        depth,
        height,
        -width,
        depth_segments,
        height_segments,
        1,
    );
    build_plane(
        g,
        (x, z, y),
        1.0,
        1.0,
        width,
        depth,
        height,
        width_segments,
        depth_segments,
        2,
    );
    build_plane(
        g,
        (x, z, y),
        1.0,
        -1.0,
        width,
        depth,
        -height,
        width_segments,
        depth_segments,
        3,
    );
    build_plane(
        g,
        (x, y, z),
        1.0,
        -1.0,
        width,
        height,
        depth,
        width_segments,
        height_segments,
        4,
    );
    build_plane(
        g,
        (x, y, z),
        -1.0,
        -1.0,
        width,
        height,
        -depth,
        width_segments,
        height_segments,
        5,
    );
    finish(geometry, &indices, &vertices, &normals, &uvs)
}

/// `new PlaneGeometry(width, height, widthSegments, heightSegments)`.
pub fn plane_geometry(
    width: f64,
    height: f64,
    width_segments: f64,
    height_segments: f64,
) -> BufferGeometry {
    let width_half = width / 2.0;
    let height_half = height / 2.0;
    let grid_x = math::floor(width_segments);
    let grid_y = math::floor(height_segments);
    let grid_x1 = grid_x + 1.0;
    let grid_y1 = grid_y + 1.0;
    let segment_width = width / grid_x;
    let segment_height = height / grid_y;
    let mut indices = Vec::new();
    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut iy = 0.0;
    while iy < grid_y1 {
        let y = iy * segment_height - height_half;
        let mut ix = 0.0;
        while ix < grid_x1 {
            let x = ix * segment_width - width_half;
            vertices.extend_from_slice(&[x, -y, 0.0]);
            normals.extend_from_slice(&[0.0, 0.0, 1.0]);
            uvs.push(ix / grid_x);
            uvs.push(1.0 - (iy / grid_y));
            ix += 1.0;
        }
        iy += 1.0;
    }
    let mut iy = 0.0;
    while iy < grid_y {
        let mut ix = 0.0;
        while ix < grid_x {
            let a = ix + grid_x1 * iy;
            let b = ix + grid_x1 * (iy + 1.0);
            let c = (ix + 1.0) + grid_x1 * (iy + 1.0);
            let d = (ix + 1.0) + grid_x1 * iy;
            indices.extend_from_slice(&[a, b, d, b, c, d]);
            ix += 1.0;
        }
        iy += 1.0;
    }
    finish(BufferGeometry::new(), &indices, &vertices, &normals, &uvs)
}

/// `new CylinderGeometry(radiusTop, radiusBottom, height, radialSegments, heightSegments,
/// openEnded, thetaStart, thetaLength)`.
#[allow(clippy::too_many_arguments)]
pub fn cylinder_geometry(
    radius_top: f64,
    radius_bottom: f64,
    height: f64,
    radial_segments: f64,
    height_segments: f64,
    open_ended: bool,
    theta_start: f64,
    theta_length: f64,
) -> BufferGeometry {
    let radial_segments = math::floor(radial_segments);
    let height_segments = math::floor(height_segments);
    let mut geometry = BufferGeometry::new();
    let mut indices = Vec::new();
    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut index = 0.0;
    let mut index_array: Vec<Vec<f64>> = Vec::new();
    let half_height = height / 2.0;
    let mut group_start = 0.0;

    let mut group_count = 0.0;
    let slope = (radius_bottom - radius_top) / height;
    let mut y = 0.0;
    while y <= height_segments {
        let mut index_row = Vec::new();
        let v = y / height_segments;
        let radius = v * (radius_bottom - radius_top) + radius_top;
        let mut x = 0.0;
        while x <= radial_segments {
            let u = x / radial_segments;
            let theta = u * theta_length + theta_start;
            let sin_theta = math::sin(theta);
            let cos_theta = math::cos(theta);
            vertices.extend_from_slice(&[
                radius * sin_theta,
                -v * height + half_height,
                radius * cos_theta,
            ]);
            let normal = Vector3::new(sin_theta, slope, cos_theta).normalize();
            normals.extend_from_slice(&normal.to_array());
            uvs.extend_from_slice(&[u, 1.0 - v]);
            index_row.push(index);
            index += 1.0;
            x += 1.0;
        }
        index_array.push(index_row);
        y += 1.0;
    }
    let at = |rows: &[Vec<f64>], y: f64, x: f64| rows[y as usize][x as usize];
    let mut x = 0.0;
    while x < radial_segments {
        let mut y = 0.0;
        while y < height_segments {
            let a = at(&index_array, y, x);
            let b = at(&index_array, y + 1.0, x);
            let c = at(&index_array, y + 1.0, x + 1.0);
            let d = at(&index_array, y, x + 1.0);
            indices.extend_from_slice(&[a, b, d, b, c, d]);
            group_count += 6.0;
            y += 1.0;
        }
        x += 1.0;
    }
    geometry.add_group(group_start as usize, group_count as usize, 0);
    group_start += group_count;

    let mut generate_cap = |geometry: &mut BufferGeometry, top: bool| {
        let center_index_start = index;
        let mut group_count = 0.0;
        let radius = if top { radius_top } else { radius_bottom };
        let sign = if top { 1.0 } else { -1.0 };
        let mut x = 1.0;
        while x <= radial_segments {
            vertices.extend_from_slice(&[0.0, half_height * sign, 0.0]);
            normals.extend_from_slice(&[0.0, sign, 0.0]);
            uvs.extend_from_slice(&[0.5, 0.5]);
            index += 1.0;
            x += 1.0;
        }
        let center_index_end = index;
        let mut x = 0.0;
        while x <= radial_segments {
            let u = x / radial_segments;
            let theta = u * theta_length + theta_start;
            let cos_theta = math::cos(theta);
            let sin_theta = math::sin(theta);
            vertices.extend_from_slice(&[
                radius * sin_theta,
                half_height * sign,
                radius * cos_theta,
            ]);
            normals.extend_from_slice(&[0.0, sign, 0.0]);
            uvs.extend_from_slice(&[(cos_theta * 0.5) + 0.5, (sin_theta * 0.5 * sign) + 0.5]);
            index += 1.0;
            x += 1.0;
        }
        let mut x = 0.0;
        while x < radial_segments {
            let c = center_index_start + x;
            let i = center_index_end + x;
            if top {
                indices.extend_from_slice(&[i, i + 1.0, c]);
            } else {
                indices.extend_from_slice(&[i + 1.0, i, c]);
            }
            group_count += 3.0;
            x += 1.0;
        }
        geometry.add_group(
            group_start as usize,
            group_count as usize,
            if top { 1 } else { 2 },
        );
        group_start += group_count;
    };

    if !open_ended {
        if radius_top > 0.0 {
            generate_cap(&mut geometry, true);
        }
        if radius_bottom > 0.0 {
            generate_cap(&mut geometry, false);
        }
    }
    finish(geometry, &indices, &vertices, &normals, &uvs)
}

/// `new ConeGeometry(radius, height, radialSegments, heightSegments, openEnded, thetaStart,
/// thetaLength)`.
pub fn cone_geometry(
    radius: f64,
    height: f64,
    radial_segments: f64,
    height_segments: f64,
    open_ended: bool,
    theta_start: f64,
    theta_length: f64,
) -> BufferGeometry {
    cylinder_geometry(
        0.0,
        radius,
        height,
        radial_segments,
        height_segments,
        open_ended,
        theta_start,
        theta_length,
    )
}

/// `new SphereGeometry(radius, widthSegments, heightSegments, phiStart, phiLength, thetaStart,
/// thetaLength)`.
pub fn sphere_geometry(
    radius: f64,
    width_segments: f64,
    height_segments: f64,
    phi_start: f64,
    phi_length: f64,
    theta_start: f64,
    theta_length: f64,
) -> BufferGeometry {
    let width_segments = math::max2(3.0, math::floor(width_segments));
    let height_segments = math::max2(2.0, math::floor(height_segments));
    let theta_end = math::min2(theta_start + theta_length, math::PI);
    let mut index = 0.0;
    let mut grid: Vec<Vec<f64>> = Vec::new();
    let mut indices = Vec::new();
    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut iy = 0.0;
    while iy <= height_segments {
        let mut vertices_row = Vec::new();
        let v = iy / height_segments;
        let mut u_offset = 0.0;
        if iy == 0.0 && theta_start == 0.0 {
            u_offset = 0.5 / width_segments;
        } else if iy == height_segments && theta_end == math::PI {
            u_offset = -0.5 / width_segments;
        }
        let mut ix = 0.0;
        while ix <= width_segments {
            let u = ix / width_segments;
            let vertex = Vector3::new(
                -radius
                    * math::cos(phi_start + u * phi_length)
                    * math::sin(theta_start + v * theta_length),
                radius * math::cos(theta_start + v * theta_length),
                radius
                    * math::sin(phi_start + u * phi_length)
                    * math::sin(theta_start + v * theta_length),
            );
            vertices.extend_from_slice(&vertex.to_array());
            normals.extend_from_slice(&vertex.normalize().to_array());
            uvs.extend_from_slice(&[u + u_offset, 1.0 - v]);
            vertices_row.push(index);
            index += 1.0;
            ix += 1.0;
        }
        grid.push(vertices_row);
        iy += 1.0;
    }
    let at = |iy: f64, ix: f64| grid[iy as usize][ix as usize];
    let mut iy = 0.0;
    while iy < height_segments {
        let mut ix = 0.0;
        while ix < width_segments {
            let a = at(iy, ix + 1.0);
            let b = at(iy, ix);
            let c = at(iy + 1.0, ix);
            let d = at(iy + 1.0, ix + 1.0);
            if iy != 0.0 || theta_start > 0.0 {
                indices.extend_from_slice(&[a, b, d]);
            }
            if iy != height_segments - 1.0 || theta_end < math::PI {
                indices.extend_from_slice(&[b, c, d]);
            }
            ix += 1.0;
        }
        iy += 1.0;
    }
    finish(BufferGeometry::new(), &indices, &vertices, &normals, &uvs)
}
