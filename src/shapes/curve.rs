// std
use std;
use std::f32::consts::PI;
use std::sync::Arc;
// pbrt
use core::efloat::EFloat;
use core::efloat::quadratic_efloat;
use core::geometry::{Bounds3f, Normal3f, Point2f, Point3f, Ray, Vector2f, Vector3f};
use core::geometry::{nrm_dot_nrm, nrm_normalize, bnd3_expand, bnd3_union_bnd3, nrm_abs_dot_vec3,
                     pnt3_distance, pnt3_distance_squared, pnt3_lerp, pnt3_offset_ray_origin,
                     spherical_direction_vec3, vec2_dot, vec3_coordinate_system, vec3_cross_vec3,
                     vec3_dot_vec3, vec3_normalize};
use core::interaction::{Interaction, InteractionCommon, SurfaceInteraction};
use core::material::Material;
use core::pbrt::Float;
use core::pbrt::{clamp_t, float_to_bits, gamma, lerp, radians};
use core::sampling::{uniform_cone_pdf, uniform_sample_sphere};
use core::shape::Shape;
use core::transform::Transform;

// see curve.h

#[derive(Debug, Clone, PartialEq)]
pub enum CurveType {
    Flat,
    Cylinder,
    Ribbon,
}

#[derive(Clone)]
pub struct CurveCommon {
    pub curve_type: CurveType,
    pub cp_obj: [Point3f; 4],
    pub width: [Float; 2],
    pub n: [Normal3f; 2],
    pub normal_angle: Float,
    pub inv_sin_normal_angle: Float,
}

impl CurveCommon {
    pub fn new(
        c: &[Point3f; 4],
        width0: Float,
        width1: Float,
        curve_type: CurveType,
        norm: Option<[Normal3f; 2]>,
    ) -> Self {
        if let Some(norm) = norm {
            let n0: Normal3f = nrm_normalize(norm[0]);
            let n1: Normal3f = nrm_normalize(norm[1]);
            let normal_angle: Float =
                clamp_t(nrm_dot_nrm(n0, n1), 0.0 as Float, 1.0 as Float).acos();
            let inv_sin_normal_angle: Float = 1.0 as Float / normal_angle.sin();
            CurveCommon {
                curve_type: curve_type,
                cp_obj: [c[0], c[1], c[2], c[3]],
                width: [width0, width1],
                n: [n0, n1],
                normal_angle: normal_angle,
                inv_sin_normal_angle: inv_sin_normal_angle,
            }
        } else {
            CurveCommon {
                curve_type: curve_type,
                cp_obj: [c[0], c[1], c[2], c[3]],
                width: [width0, width1],
                n: [Normal3f::default(); 2],
                normal_angle: 0.0 as Float,
                inv_sin_normal_angle: 0.0 as Float,
            }
        }
    }
}

#[derive(Clone)]
pub struct Curve {
    pub common: Arc<CurveCommon>,
    pub u_min: Float,
    pub u_max: Float,
    // inherited from class Shape (see shape.h)
    object_to_world: Transform,
    world_to_object: Transform,
    reverse_orientation: bool,
    transform_swaps_handedness: bool,
    pub material: Option<Arc<Material + Send + Sync>>,
}

impl Curve {
    pub fn new(
        object_to_world: Transform,
        world_to_object: Transform,
        reverse_orientation: bool,
        common: Arc<CurveCommon>,
        u_min: Float,
        u_max: Float,
    ) -> Self {
        Curve {
            // Curve
            common: common,
            u_min: u_min,
            u_max: u_max,
            // Shape
            object_to_world: object_to_world,
            world_to_object: world_to_object,
            reverse_orientation: reverse_orientation,
            transform_swaps_handedness: false,
            material: None,
        }
    }
    pub fn create(
        o2w: Transform,
        w2o: Transform,
        reverse_orientation: bool,
        c: &[Point3f; 4],
        w0: Float,
        w1: Float,
        curve_type: CurveType,
        norm: Option<[Normal3f; 2]>,
        split_depth: i32,
    ) -> Vec<Arc<Shape + Send + Sync>> {
        let common: Arc<CurveCommon> = Arc::new(CurveCommon::new(c, w0, w1, curve_type, norm));
        let n_segments: usize = 1_usize << split_depth;
        let mut segments: Vec<Arc<Shape + Send + Sync>> = Vec::with_capacity(n_segments);
        for i in 0..n_segments {
            let u_min: Float = i as Float / n_segments as Float;
            let u_max: Float = (i + 1) as Float / n_segments as Float;
            // segments.push_back(std::make_shared<Curve>(o2w, w2o, reverseOrientation,
            //                                            common, u_min, u_max));
            let curve: Arc<Curve> = Arc::new(Curve::new(
                o2w,
                w2o,
                reverse_orientation,
                common.clone(),
                u_min,
                u_max,
            ));
            segments.push(curve.clone());
            // TODO: ++nSplitCurves;
        }
        // TODO: curveBytes += sizeof(CurveCommon) + n_segments * sizeof(Curve);
        segments
    }
    fn recursive_intersect(
        &self,
        ray: &Ray,
        cp: &[Point3f; 4],
        ray_to_object: &Transform,
        u0: Float,
        u1: Float,
        depth: i32,
    ) -> Option<(SurfaceInteraction, Float)> {
        let ray_length: Float = ray.d.length();

        if depth > 0_i32 {
            // split curve segment into sub-segments and test for intersection
            let mut cp_split: [Point3f; 7] = [Point3f::default(); 7];
            subdivide_bezier(cp, &mut cp_split);

            // For each of the two segments, see if the ray's bounding
            // box overlaps the segment before recursively checking
            // for intersection with it.

            let hit: bool = false;
            let u: [Float; 3] = [u0, (u0 + u1) / 2.0 as Float, u1];
            // pointer to the 4 control points for the current segment.
            //     const Point3f *cps = cp_split;
            // for (int seg = 0; seg < 2; ++seg, cps += 3) {
            for seg in 0..2 {
                let cps: &[Point3f] = &cp_split[seg * 3..seg * 3 + 3];
                let max_width: Float = lerp(u[seg], self.common.width[0], self.common.width[1])
                    .max(lerp(u[seg + 1], self.common.width[0], self.common.width[1]));

                // As above, check y first, since it most commonly
                // lets us exit out early.

                if cps[0].y.max(cps[1].y).max(cps[2].y.max(cps[3].y)) + 0.5 as Float * max_width
                    < 0.0 as Float
                    || cps[0].y.min(cps[1].y).min(cps[2].y.min(cps[3].y)) - 0.5 as Float * max_width
                        > 0.0 as Float
                {
                    continue;
                }

                if cps[0].x.max(cps[1].x).max(cps[2].x.max(cps[3].x)) + 0.5 as Float * max_width
                    < 0.0 as Float
                    || cps[0].x.min(cps[1].x).min(cps[2].x.min(cps[3].x)) - 0.5 as Float * max_width
                        > 0.0 as Float
                {
                    continue;
                }

                let z_max: Float = ray_length * ray.t_max;
                if cps[0].z.max(cps[1].z).max(cps[2].z.max(cps[3].z)) + 0.5 as Float * max_width
                    < 0.0 as Float
                    || cps[0].z.min(cps[1].z).min(cps[2].z.min(cps[3].z)) - 0.5 as Float * max_width
                        > z_max
                {
                    continue;
                }

                //     hit |= recursiveIntersect(ray, tHit, isect, cps, rayToObject,
                //                               u[seg], u[seg + 1], depth - 1);

                // let hit_option = self.recursive_intersect(
                //     ray,
                //     &cps[0..4],
                //     ray_to_object,
                //     u[seg],
                //     u[seg + 1],
                //     depth - 1,
                // );

                //     // If we found an intersection and this is a shadow ray,
                //     // we can exit out immediately.
                //     if (hit && !tHit) return true;
            }
        //     return hit;
        } else {
            // intersect ray with curve segment

            // test ray against segment endpoint boundaries

            // test sample point against tangent perpendicular at curve start
            let mut edge: Float = (cp[1].y - cp[0].y) * -cp[0].y + cp[0].x * (cp[0].x - cp[1].x);
            if edge < 0.0 as Float {
                return None;
            }

            // test sample point against tangent perpendicular at curve end
            edge = (cp[2].y - cp[3].y) * -cp[3].y + cp[3].x * (cp[3].x - cp[2].x);
            if edge < 0.0 as Float {
                return None;
            }

            // compute line $w$ that gives minimum distance to sample point
            let segment_direction: Vector2f = Point2f {
                x: cp[3].x,
                y: cp[3].y,
            } - Point2f {
                x: cp[0].x,
                y: cp[0].y,
            };
            let denom: Float = segment_direction.length_squared();
            if denom == 0.0 as Float {
                return None;
            }
            let w: Float = vec2_dot(
                -Vector2f {
                    x: cp[0].x,
                    y: cp[0].y,
                },
                segment_direction,
            ) / denom;

            // compute $u$ coordinate of curve intersection point and _hitWidth_
            let u: Float = clamp_t(lerp(w, u0, u1), u0, u1);
            let hit_width: Float = lerp(u, self.common.width[0], self.common.width[1]);
            let mut n_hit: Normal3f = Normal3f::default();
            if self.common.curve_type == CurveType::Ribbon {
                // scale _hitWidth_ based on ribbon orientation
                let sin0: Float = ((1.0 as Float - u) * self.common.normal_angle).sin()
                    * self.common.inv_sin_normal_angle;
                let sin1: Float =
                    (u * self.common.normal_angle).sin() * self.common.inv_sin_normal_angle;
                n_hit = self.common.n[0] * sin0 + self.common.n[1] * sin1;
                // hit_width *= AbsDot(n_hit, ray.d) / ray_length;
            }

            // test intersection point against curve width
            let mut dpcdw: Vector3f = Vector3f::default();
            let pc: Point3f =
                eval_bezier(cp, clamp_t(w, 0.0 as Float, 1.0 as Float), Some(&mut dpcdw));
            let pt_curve_dist2: Float = pc.x * pc.x + pc.y * pc.y;
            if pt_curve_dist2 > hit_width * hit_width * 0.25 as Float {
                return None;
            }
            let z_max: Float = ray_length * ray.t_max;
            if pc.z < 0.0 as Float || pc.z > z_max {
                return None;
            }

            // compute $v$ coordinate of curve intersection point
            let pt_curve_dist: Float = pt_curve_dist2.sqrt();
            let edge_func: Float = dpcdw.x * -pc.y + pc.x * dpcdw.y;
            let v: Float;
            if edge_func > 0.0 as Float {
                v = 0.5 as Float + pt_curve_dist / hit_width;
            } else {
                v = 0.5 as Float - pt_curve_dist / hit_width;
            }

            // compute hit _t_ and partial derivatives for curve intersection
            //     if (tHit != nullptr) {
            //         // FIXME: this tHit isn't quite right for ribbons...
            //         *tHit = pc.z / ray_length;
            //         // Compute error bounds for curve intersection
            //         Vector3f pError(2 * hit_width, 2 * hit_width, 2 * hit_width);

            //         // Compute $\dpdu$ and $\dpdv$ for curve intersection
            //         Vector3f dpdu, dpdv;
            //         eval_bezier(self.common.cpObj, u, &dpdu);
            //         if (self.common.type == CurveType::Ribbon)
            //             dpdv = Normalize(Cross(n_hit, dpdu)) * hit_width;
            //         else {
            //             // Compute curve $\dpdv$ for flat and cylinder curves
            //             Vector3f dpduPlane = (Inverse(rayToObject))(dpdu);
            //             Vector3f dpdvPlane =
            //                 Normalize(Vector3f(-dpduPlane.y, dpduPlane.x, 0)) *
            //                 hit_width;
            //             if (self.common.type == CurveType::Cylinder) {
            //                 // Rotate _dpdvPlane_ to give cylindrical appearance
            //                 Float theta = Lerp(v, -90., 90.);
            //                 Transform rot = Rotate(-theta, dpduPlane);
            //                 dpdvPlane = rot(dpdvPlane);
            //             }
            //             dpdv = rayToObject(dpdvPlane);
            //         }
            //         *isect = (*ObjectToWorld)(SurfaceInteraction(
            //             ray(pc.z), pError, Point2f(u, v), -ray.d, dpdu, dpdv,
            //             Normal3f(0, 0, 0), Normal3f(0, 0, 0), ray.time, this));
            //     }
            //     ++n_hits;
            //     return true;
        }
        None
    }
}

impl Shape for Curve {
    fn object_bound(&self) -> Bounds3f {
        // compute object-space control points for curve segment, _cpObj_
        let mut cp_obj: [Point3f; 4] = [Point3f::default(); 4];
        cp_obj[0] = blossom_bezier(&self.common.cp_obj, self.u_min, self.u_min, self.u_min);
        cp_obj[1] = blossom_bezier(&self.common.cp_obj, self.u_min, self.u_min, self.u_max);
        cp_obj[2] = blossom_bezier(&self.common.cp_obj, self.u_min, self.u_max, self.u_max);
        cp_obj[3] = blossom_bezier(&self.common.cp_obj, self.u_max, self.u_max, self.u_max);
        let b: Bounds3f = bnd3_union_bnd3(
            Bounds3f::new(cp_obj[0], cp_obj[1]),
            Bounds3f::new(cp_obj[2], cp_obj[3]),
        );
        let width: [Float; 2] = [
            lerp(self.u_min, self.common.width[0], self.common.width[1]),
            lerp(self.u_max, self.common.width[0], self.common.width[1]),
        ];
        bnd3_expand(b, width[0].max(width[1]) * 0.5 as Float)
    }
    fn world_bound(&self) -> Bounds3f {
        // in C++: Bounds3f Shape::WorldBound() const { return (*ObjectToWorld)(ObjectBound()); }
        self.object_to_world.transform_bounds(self.object_bound())
    }
    fn intersect(&self, r: &Ray) -> Option<(SurfaceInteraction, Float)> {
        // TODO: ProfilePhase p(isect ? Prof::CurveIntersect : Prof::CurveIntersectP);
        // TODO: ++nTests;
        // transform _Ray_ to object space
        let mut o_err: Vector3f = Vector3f::default();
        let mut d_err: Vector3f = Vector3f::default();
        let ray: Ray = self.world_to_object
            .transform_ray_with_error(r, &mut o_err, &mut d_err);

        // compute object-space control points for curve segment, _cpObj_

        let mut cp_obj: [Point3f; 4] = [Point3f::default(); 4];
        cp_obj[0] = blossom_bezier(&self.common.cp_obj, self.u_min, self.u_min, self.u_min);
        cp_obj[1] = blossom_bezier(&self.common.cp_obj, self.u_min, self.u_min, self.u_max);
        cp_obj[2] = blossom_bezier(&self.common.cp_obj, self.u_min, self.u_max, self.u_max);
        cp_obj[3] = blossom_bezier(&self.common.cp_obj, self.u_max, self.u_max, self.u_max);

        // project curve control points to plane perpendicular to ray

        // Be careful to set the "up" direction passed to LookAt() to
        // equal the vector from the first to the last control points.
        // In turn, this helps orient the curve to be roughly parallel
        // to the x axis in the ray coordinate system.

        // In turn (especially for curves that are approaching stright
        // lines), we get curve bounds with minimal extent in y, which
        // in turn lets us early out more quickly in
        // recursiveIntersect().

        // Vector3f dx = Cross(ray.d, cpObj[3] - cpObj[0]);
        let mut dx: Vector3f = vec3_cross_vec3(ray.d, cp_obj[3] - cp_obj[0]);
        if dx.length_squared() == 0.0 as Float {
            // if the ray and the vector between the first and last
            // control points are parallel, dx will be zero.  Generate
            // an arbitrary xy orientation for the ray coordinate
            // system so that intersection tests can proceeed in this
            // unusual case.
            let mut dy: Vector3f = Vector3f::default();
            vec3_coordinate_system(&ray.d, &mut dx, &mut dy);
        }

        let object_to_ray: Transform = Transform::look_at(ray.o, ray.o + ray.d, dx);
        let mut cp: [Point3f; 4] = [
            object_to_ray.transform_point(cp_obj[0]),
            object_to_ray.transform_point(cp_obj[1]),
            object_to_ray.transform_point(cp_obj[2]),
            object_to_ray.transform_point(cp_obj[3]),
        ];

        // Before going any further, see if the ray's bounding box
        // intersects the curve's bounding box. We start with the y
        // dimension, since the y extent is generally the smallest
        // (and is often tiny) due to our careful orientation of the
        // ray coordinate ysstem above.

        let max_width: Float = lerp(self.u_min, self.common.width[0], self.common.width[1])
            .max(lerp(self.u_max, self.common.width[0], self.common.width[1]));
        if cp[0].y.max(cp[1].y).max(cp[2].y.max(cp[3].y)) + 0.5 as Float * max_width < 0.0 as Float
            || cp[0].y.min(cp[1].y).min(cp[2].y.min(cp[3].y)) - 0.5 as Float * max_width
                > 0.0 as Float
        {
            return None;
        }

        // check for non-overlap in x.
        if cp[0].x.max(cp[1].x).max(cp[2].x.max(cp[3].x)) + 0.5 as Float * max_width < 0.0 as Float
            || cp[0].x.min(cp[1].x).min(cp[2].x.min(cp[3].x)) - 0.5 as Float * max_width
                > 0.0 as Float
        {
            return None;
        }

        // check for non-overlap in z.
        let ray_length: Float = ray.d.length();
        let z_max: Float = ray_length * ray.t_max;
        if cp[0].z.max(cp[1].z).max(cp[2].z.max(cp[3].z)) + 0.5 as Float * max_width < 0.0 as Float
            || cp[0].z.min(cp[1].z).min(cp[2].z.min(cp[3].z)) - 0.5 as Float * max_width > z_max
        {
            return None;
        }

        // compute refinement depth for curve, _maxDepth_
        let mut l0: Float = 0.0 as Float;
        for i in 0..2 {
            l0 = l0.max(
                ((cp[i].x - 2.0 as Float * cp[i + 1].x + cp[i + 2].x)
                    .abs()
                    .max((cp[i].y - 2.0 as Float * cp[i + 1].y + cp[i + 2].y).abs())
                    .max((cp[i].z - 2.0 as Float * cp[i + 1].z + cp[i + 2].z).abs())),
            );
        }

        // width / 20
        let eps: Float = self.common.width[0].max(self.common.width[1]) * 0.05 as Float;
        // compute log base 4 by dividing log2 in half.
        let r0: i32 =
            log2(1.41421356237 as Float * 6.0 as Float * l0 / (8.0 as Float * eps)) / 2_i32;
        let max_depth: i32 = clamp_t(r0, 0_i32, 10_i32);
        // TODO: ReportValue(refinementLevel, maxDepth);

        // return recursiveIntersect(ray, tHit, isect, cp, Inverse(object_to_ray), uMin,
        //                           uMax, maxDepth);
        // TODO
        None
    }
    fn intersect_p(&self, r: &Ray) -> bool {
        // TODO
        false
    }
    fn get_reverse_orientation(&self) -> bool {
        self.reverse_orientation
    }
    fn get_transform_swaps_handedness(&self) -> bool {
        self.transform_swaps_handedness
    }
    fn area(&self) -> Float {
        // compute object-space control points for curve segment, _cpObj_
        let mut cp_obj: [Point3f; 4] = [Point3f::default(); 4];
        cp_obj[0] = blossom_bezier(&self.common.cp_obj, self.u_min, self.u_min, self.u_min);
        cp_obj[1] = blossom_bezier(&self.common.cp_obj, self.u_min, self.u_min, self.u_max);
        cp_obj[2] = blossom_bezier(&self.common.cp_obj, self.u_min, self.u_max, self.u_max);
        cp_obj[3] = blossom_bezier(&self.common.cp_obj, self.u_max, self.u_max, self.u_max);
        let width0: Float = lerp(self.u_min, self.common.width[0], self.common.width[1]);
        let width1: Float = lerp(self.u_max, self.common.width[0], self.common.width[1]);
        let avg_width: Float = (width0 + width1) * 0.5 as Float;
        let mut approx_length: Float = 0.0 as Float;
        for i in 0..3 {
            approx_length += pnt3_distance(cp_obj[i], cp_obj[i + 1]);
        }
        approx_length * avg_width
    }
    fn sample(&self, u: Point2f, pdf: &mut Float) -> InteractionCommon {
        println!("FATAL: Curve::sample not implemented.");
        InteractionCommon::default()
    }
    fn sample_with_ref_point(
        &self,
        iref: &InteractionCommon,
        u: Point2f,
        pdf: &mut Float,
    ) -> InteractionCommon {
        let intr: InteractionCommon = self.sample(u, pdf);
        let mut wi: Vector3f = intr.p - iref.p;
        if wi.length_squared() == 0.0 as Float {
            *pdf = 0.0 as Float;
        } else {
            wi = vec3_normalize(wi);
            // convert from area measure, as returned by the Sample()
            // call above, to solid angle measure.
            *pdf *= pnt3_distance_squared(iref.p, intr.p) / nrm_abs_dot_vec3(intr.n, -wi);
            if (*pdf).is_infinite() {
                *pdf = 0.0 as Float;
            }
        }
        intr
    }
    fn pdf(&self, iref: &Interaction, wi: Vector3f) -> Float {
        // intersect sample ray with area light geometry
        let ray: Ray = iref.spawn_ray(wi);
        // ignore any alpha textures used for trimming the shape when
        // performing this intersection. Hack for the "San Miguel"
        // scene, where this is used to make an invisible area light.
        if let Some((isect_light, _t_hit)) = self.intersect(&ray) {
            // convert light sample weight to solid angle measure
            let mut pdf: Float = pnt3_distance_squared(iref.get_p(), isect_light.p)
                / (nrm_abs_dot_vec3(isect_light.n, -wi) * self.area());
            if pdf.is_infinite() {
                pdf = 0.0 as Float;
            }
            pdf
        } else {
            0.0 as Float
        }
    }
}

// Curve Utility Functions

fn blossom_bezier(p: &[Point3f; 4], u0: Float, u1: Float, u2: Float) -> Point3f {
    let a: [Point3f; 3] = [
        pnt3_lerp(u0, p[0], p[1]),
        pnt3_lerp(u0, p[1], p[2]),
        pnt3_lerp(u0, p[2], p[3]),
    ];
    let b: [Point3f; 2] = [pnt3_lerp(u1, a[0], a[1]), pnt3_lerp(u1, a[1], a[2])];
    pnt3_lerp(u2, b[0], b[1])
}

fn subdivide_bezier(cp: &[Point3f; 4], cp_split: &mut [Point3f; 7]) {
    cp_split[0] = cp[0];
    cp_split[1] = (cp[0] + cp[1]) / 2.0 as Float;
    cp_split[2] = (cp[0] + cp[1] * 2.0 as Float + cp[2]) / 4.0 as Float;
    cp_split[3] = (cp[0] + cp[1] * 3.0 as Float + cp[2] * 3.0 as Float + cp[3]) / 8.0 as Float;
    cp_split[4] = (cp[1] + cp[2] * 2.0 as Float + cp[3]) / 4.0 as Float;
    cp_split[5] = (cp[2] + cp[3]) / 2.0 as Float;
    cp_split[6] = cp[3];
}

fn eval_bezier(cp: &[Point3f; 4], u: Float, deriv: Option<&mut Vector3f>) -> Point3f {
    let cp1: [Point3f; 3] = [
        pnt3_lerp(u, cp[0], cp[1]),
        pnt3_lerp(u, cp[1], cp[2]),
        pnt3_lerp(u, cp[2], cp[3]),
    ];
    let cp2: [Point3f; 2] = [pnt3_lerp(u, cp1[0], cp1[1]), pnt3_lerp(u, cp1[1], cp1[2])];
    if let Some(deriv) = deriv {
        *deriv = (cp2[1] - cp2[0]) * 3.0 as Float;
    }
    pnt3_lerp(u, cp2[0], cp2[1])
}

fn log2(v: Float) -> i32 {
    if v < 1.0 as Float {
        return 0_i32;
    }
    let bits: i32 = float_to_bits(v) as i32;

    // https://graphics.stanford.edu/~seander/bithacks.html#IntegerLog

    // (With an additional add so get round-to-nearest rather than
    // round down.)
    let mut one_or_zero: i32 = 0_i32;
    if (1 << 22) > 0 {
        one_or_zero = 1_i32;
    }
    (bits >> 23) - 127 + (bits & one_or_zero)
}
