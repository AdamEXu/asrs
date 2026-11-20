use super::gpu::AuxBuffers;

/// Depth discontinuity threshold on the second difference of window depth. Window depth is
/// affine in screen space across a plane, so smooth surfaces stay near zero and silhouettes of
/// an object against itself stand out.
const DEPTH_SECOND_DIFFERENCE: f32 = 2e-5;

/// Geometry edges for the 3D pixel gate (section 2.5, gate 4): pixels whose 8-neighbourhood
/// holds another object id (opaque or transparent layer) or a depth discontinuity within one
/// object. The gate then treats everything within 2 px of these as edge zone.
pub fn edge_mask(aux: &AuxBuffers) -> Vec<bool> {
    let (w, h) = (aux.width as usize, aux.height as usize);
    let mut edges = vec![false; w * h];
    let at = |x: usize, y: usize| y * w + x;
    for y in 0..h {
        for x in 0..w {
            let i = at(x, y);
            let mut edge = false;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = at(nx as usize, ny as usize);
                    edge |= aux.opaque_ids[i] != aux.opaque_ids[j]
                        || aux.transparent_ids[i] != aux.transparent_ids[j];
                }
            }
            if !edge && aux.opaque_ids[i] != 0 {
                let d = &aux.depth;
                if x > 0 && x + 1 < w {
                    edge |= (d[at(x - 1, y)] - 2.0 * d[i] + d[at(x + 1, y)]).abs()
                        > DEPTH_SECOND_DIFFERENCE;
                }
                if y > 0 && y + 1 < h {
                    edge |= (d[at(x, y - 1)] - 2.0 * d[i] + d[at(x, y + 1)]).abs()
                        > DEPTH_SECOND_DIFFERENCE;
                }
            }
            edges[i] = edge;
        }
    }
    edges
}
