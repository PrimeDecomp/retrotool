use super::*;

fn projection(axis: ProjectionAxis) -> DecalProjection {
    DecalProjection::from_box(
        CTransform4f::default(),
        [2.0, 3.0, 4.0],
        axis,
        std::f32::consts::FRAC_PI_4,
    )
    .unwrap()
}

#[test]
fn original_arm64_projection_axes_and_plane_order() {
    let y = projection(ProjectionAxis::Y);
    assert_eq!(y.origin, [2.0, -3.0, -4.0]);
    assert_eq!(y.u_axis, [-0.25, 0.0, 0.0]);
    assert_eq!(y.v_axis, [0.0, 0.0, 0.125]);
    assert_eq!(y.projection_plane, [0.0, 1.0, 0.0, -3.0]);
    assert_eq!(y.clip_planes, [
        [-1.0, 0.0, 0.0, -2.0],
        [0.0, 0.0, 1.0, -4.0],
        [1.0, 0.0, 0.0, -2.0],
        [0.0, 0.0, -1.0, -4.0],
        [0.0, -1.0, 0.0, -3.0],
        [0.0, 1.0, 0.0, -3.0],
    ]);
    let z = projection(ProjectionAxis::Z);
    assert_eq!(z.origin, y.origin);
    assert_eq!(z.u_axis, y.u_axis);
    assert_eq!(z.v_axis, [0.0, 1.0 / 6.0, 0.0]);
    assert_eq!(z.projection_plane, [0.0, 0.0, 1.0, -4.0]);
    assert_eq!(z.clip_planes[0], y.clip_planes[4]);
    assert_eq!(z.clip_planes[5], y.clip_planes[3]);
    let words = y.uniform_words();
    assert_eq!(&words[28..32], &y.projection_plane);
    assert_eq!(&words[32..36], &[2.0, -3.0, -4.0, 1.0]);
    assert_eq!(words[44], y.minimum_normal_dot);
}

#[test]
fn original_nvn_triangle_output_and_mirrored_winding() {
    // Captured from source16 through Eden and NVIDIA transform feedback.
    let projection = projection(ProjectionAxis::Y);
    let positions = [[-1.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, -1.0]];
    let vertices = projection.project_triangle(positions, false).unwrap();
    assert_eq!(vertices.map(|v| v.position), positions);
    assert_eq!(vertices.map(|v| v.uv), [[0.75, 0.375], [0.5, 0.625], [0.25, 0.375]]);
    assert_eq!(vertices[0].clip_distances, [3.0, 3.0, 1.0, 5.0, 3.0, 3.0]);
    assert!(projection.project_triangle(positions, true).is_none());
    let reversed = [positions[0], positions[2], positions[1]];
    assert!(projection.project_triangle(reversed, false).is_none());
    let mirrored = projection.project_triangle(reversed, true).unwrap();
    assert_eq!(mirrored.map(|v| v.position), positions);
}

#[test]
fn only_wholly_outside_triangles_are_rejected_and_boundaries_are_inclusive() {
    let projection = projection(ProjectionAxis::Y);
    let positions = [[-1.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, -1.0]];
    for axis in 0..3 {
        for shift in [-20.0, 20.0] {
            let moved = positions.map(|mut point| {
                point[axis] += shift;
                point
            });
            assert!(projection.project_triangle(moved, false).is_none());
        }
    }
    let spanning = [[-10.0, 0.0, -10.0], [0.0, 0.0, 10.0], [10.0, 0.0, -10.0]];
    let vertices = projection.project_triangle(spanning, false).unwrap();
    assert_eq!(vertices.map(|v| v.position), spanning);
    assert!(vertices.iter().all(|v| v.clip_distances.iter().any(|distance| *distance < 0.0)));
    for height in [-3.0, 3.0] {
        let on_plane = positions.map(|[x, _, z]| [x, height, z]);
        assert!(projection.project_triangle(on_plane, false).is_some());
    }
}

#[test]
fn native_degenerate_geometry_is_distinct_from_invalid_box_inputs() {
    // A zero normal becomes NaN in the geometry shader. Its ordered angle test
    // does not reject, and the finite positions/UV/clip distances are emitted.
    assert!(projection(ProjectionAxis::Y).project_triangle([[0.0; 3]; 3], false).is_some());
    let tiny = DecalProjection::from_box(
        CTransform4f::default(),
        [0.001, 0.005, 0.01],
        ProjectionAxis::Y,
        0.0,
    )
    .unwrap();
    assert_eq!(tiny.u_axis, [-0.002, 0.0, 0.0]);
    assert_eq!(tiny.v_axis, [0.0, 0.0, 0.02]);
    assert!(DecalProjection::from_box(
        CTransform4f::default(),
        [-1.0, 1.0, 1.0],
        ProjectionAxis::Y,
        0.0,
    )
    .is_err());
    assert!(DecalProjection::from_box(
        CTransform4f::default(),
        [1.0; 3],
        ProjectionAxis::Y,
        f32::NAN,
    )
    .is_err());
}
