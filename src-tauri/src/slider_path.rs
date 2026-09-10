// slider_path.rs — v128 多段混合曲线 → stable 单 B| bezier 锚点降级。
//
// 背景（调研 2026-09-05）：stable 对整条 slider 只认最后出现的曲线类型
// （ppy/osu LegacyBeatmapExporter.cs 注释原文），lazer v128 允许单条 slider
// 多段混合类型（P|…|L|…），直出 stable 会整条变形（ppy/osu#31713/#24534）。
// 官方解法（MutateBeatmap）与 LazerToStable 一致：全部段转 bezier 锚点、
// 单 B| 前缀、段边界 double-up（stable 以连续重复点切分 bezier 子段）、
// 末控制点裸 type 清除（#24570）、坐标 clamp ±131072 后取整。
// 圆弧近似算法移植自 Olibomby BezierConverter / osu!framework
// （单位圆弧模板 + de Casteljau 子弧收敛），与 LazerToStable 同源。

const COORD_MAX: f64 = 131_072.0;
const TAU: f64 = std::f64::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegKind {
    Bezier,
    Linear,
    Perfect,
    Catmull,
}

#[derive(Debug, Clone)]
struct Segment {
    kind: SegKind,
    points: Vec<(f64, f64)>,
}

/// 单位圆弧 bezier 近似模板（从 (1,0) 出发、逆时针、覆盖 MaxAngle 弧度）。
/// 数值与 Olibomby/LazerToStable 一致。
const CIRCLE_PRESETS: &[(f64, &[(f64, f64)])] = &[
    (
        0.4993379862754501,
        &[(1.0, 0.0), (1.0, 0.25499), (0.87790, 0.47884)],
    ),
    (
        1.7579419829169447,
        &[
            (1.0, 0.0),
            (1.0, 0.62630),
            (0.42931, 1.09907),
            (-0.18606, 0.98254),
        ],
    ),
    (
        3.1385246920140215,
        &[
            (1.0, 0.0),
            (1.0, 0.87085),
            (0.00230, 1.50331),
            (-0.99732, 0.87391),
            (-0.9999953, 0.00307),
        ],
    ),
    (
        5.69720464620727,
        &[
            (1.0, 0.0),
            (1.0, 1.41378),
            (-1.43052, 2.07794),
            (-2.34101, -0.94018),
            (0.05133, -1.73093),
            (0.83317, -0.55302),
        ],
    ),
    (
        TAU,
        &[
            (1.0, 0.0),
            (1.0, 1.24471),
            (-0.85265, 2.11837),
            (-2.62110, 0.0),
            (-0.85264, -2.11836),
            (1.0, -1.24471),
            (1.0, 0.0),
        ],
    ),
];

fn seg_kind_of(token: &str) -> Option<SegKind> {
    match token {
        "B" | "b" => Some(SegKind::Bezier),
        "L" | "l" => Some(SegKind::Linear),
        "P" | "p" => Some(SegKind::Perfect),
        "C" | "c" => Some(SegKind::Catmull),
        _ => None,
    }
}

fn parse_point(token: &str) -> Option<(f64, f64)> {
    let (x, y) = token.split_once(':')?;
    let x: f64 = x.trim().parse().ok()?;
    let y: f64 = y.trim().parse().ok()?;
    Some((
        x.clamp(-COORD_MAX, COORD_MAX),
        y.clamp(-COORD_MAX, COORD_MAX),
    ))
}

/// 解析曲线字段为显式段列表：
/// - 类型 token（B/L/P/C）开启新显式段，新段起点继承前段末点；
/// - BEZIER 段内连续重复点为隐式段边界（stable 的 red anchor 语义），拆分子段；
/// - 末尾裸类型 token 丢弃（ppy/osu#24570）；
/// - 不足 2 点的段丢弃。解析失败返回 None（调用方保持原样）。
fn parse_segments(curve: &str) -> Option<(Vec<Segment>, bool)> {
    let tokens: Vec<&str> = curve.split('|').collect();
    if tokens.is_empty() {
        return None;
    }
    // 末尾裸类型 token：无 ':' 的单字符类型（#24570）
    let mut tokens = tokens;
    let mut dropped_trailing_type = false;
    if tokens.len() > 1 && tokens.last().is_some_and(|t| seg_kind_of(t).is_some()) {
        tokens.pop();
        dropped_trailing_type = true;
    }
    let kind = seg_kind_of(tokens[0])?;
    let mut segments: Vec<Segment> = vec![Segment {
        kind,
        points: Vec::new(),
    }];
    let mut last: Option<(f64, f64)> = None;
    for t in &tokens[1..] {
        if let Some(k) = seg_kind_of(t) {
            // 新显式段：起点继承前段末点
            let mut points = Vec::new();
            if let Some(p) = last {
                points.push(p);
            }
            segments.push(Segment { kind: k, points });
            continue;
        }
        let p = parse_point(t)?;
        let cur = segments.last_mut()?;
        // BEZIER 段内重复点 = 隐式段边界，拆分子段（首点沿用）
        if cur.kind == SegKind::Bezier && cur.points.len() >= 2 && cur.points.last() == Some(&p) {
            let mut_points = &mut cur.points;
            let boundary = *mut_points.last().unwrap();
            segments.push(Segment {
                kind: SegKind::Bezier,
                points: vec![boundary],
            });
        }
        segments.last_mut()?.points.push(p);
        last = Some(p);
    }
    segments.retain(|s| s.points.len() >= 2);
    if segments.is_empty() {
        None
    } else {
        Some((segments, dropped_trailing_type))
    }
}

fn is_collinear(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> bool {
    ((b.1 - a.1) * (c.0 - a.0) - (b.0 - a.0) * (c.1 - a.1)).abs() < 1e-6
}

/// LINEAR → bezier 锚点：每个点重复一次编码直线段边界。
fn linear_anchors(pts: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(pts.len() * 2 - 1);
    for i in 0..pts.len() - 1 {
        out.push(pts[i]);
        out.push(pts[i + 1]);
    }
    out.push(pts[pts.len() - 1]);
    out
}

/// CATMULL → cubic bezier 锚点（Catmull-Rom 公式，段边界 double-up）。
fn catmull_anchors(pts: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let n = pts.len();
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n - 1 {
        let v1 = if i > 0 { pts[i - 1] } else { pts[0] };
        let v2 = pts[i];
        let v3 = pts[i + 1];
        let v4 = if i + 2 < n {
            pts[i + 2]
        } else {
            (v3.0 * 2.0 - v2.0, v3.1 * 2.0 - v2.1)
        };
        out.push(v2);
        out.push((
            (-v1.0 + 6.0 * v2.0 + v3.0) / 6.0,
            (-v1.1 + 6.0 * v2.1 + v3.1) / 6.0,
        ));
        out.push((
            (-v4.0 + 6.0 * v3.0 + v2.0) / 6.0,
            (-v4.1 + 6.0 * v3.1 + v2.1) / 6.0,
        ));
        out.push(v3);
        if i + 2 < n {
            out.push(v3); // 段边界 double-up（red anchor）
        }
    }
    out
}

/// PERFECT（三点圆弧）→ bezier 锚点：三点定圆 → 模板选取 → de Casteljau
/// 子弧收敛 → 旋转/缩放/平移 → 端点钉回原始端点。
fn circle_anchors(pts: &[(f64, f64)]) -> Option<Vec<(f64, f64)>> {
    let (ax, ay) = pts[0];
    let (bx, by) = pts[1];
    let (cx, cy) = pts[2];
    // 三点定圆（外接圆公式）
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d.abs() < 1e-9 {
        return None;
    }
    let a2 = ax * ax + ay * ay;
    let b2 = bx * bx + by * by;
    let c2 = cx * cx + cy * cy;
    let ux = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d;
    let uy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d;
    let radius = ((ax - ux).powi(2) + (ay - uy).powi(2)).sqrt();
    if !(radius.is_finite()) || radius < 1e-6 {
        return None;
    }
    let theta_start = (ay - uy).atan2(ax - ux);
    let mut theta_end = (cy - uy).atan2(cx - ux);
    while theta_end < theta_start {
        theta_end += TAU;
    }
    let mut theta_range = theta_end - theta_start;
    // 方向：b 在弦 a→c 的哪一侧决定顺/逆时针
    let cross = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
    let dir: f64 = if cross < 0.0 {
        theta_range = TAU - theta_range;
        -1.0
    } else {
        1.0
    };
    if !(theta_range.is_finite()) || theta_range < 1e-6 {
        return None;
    }
    // 模板选取：第一个 MaxAngle ≥ theta_range 的
    let (max_angle, template) = CIRCLE_PRESETS
        .iter()
        .find(|(m, _)| *m >= theta_range)
        .map(|(m, p)| (*m, p.to_vec()))
        .unwrap_or_else(|| {
            let last = CIRCLE_PRESETS.last().unwrap();
            (last.0, last.1.to_vec())
        });
    // de Casteljau 子弧收敛：反复按 tf 切割模板控制多边形，
    // 直至切割后终点角 ≈ theta_range
    let mut arc: Vec<(f64, f64)> = template.to_vec();
    let n = arc.len() - 1;
    let mut tf = theta_range / max_angle;
    for _ in 0..100 {
        for _ in 0..n {
            for i in (1..arc.len()).rev() {
                arc[i] = (
                    arc[i].0 * tf + arc[i - 1].0 * (1.0 - tf),
                    arc[i].1 * tf + arc[i - 1].1 * (1.0 - tf),
                );
            }
        }
        let end_angle = arc.last().unwrap().1.atan2(arc.last().unwrap().0);
        let mut arc_angle = end_angle;
        if arc_angle < 0.0 {
            arc_angle += TAU;
        }
        if arc_angle < 1e-9 {
            break;
        }
        let new_tf = theta_range / arc_angle;
        if (new_tf - 1.0).abs() <= 1e-6 {
            break;
        }
        tf = new_tf;
    }
    // 仿射：单位圆 → 半径缩放 × θstart 旋转 × 方向翻转 × 圆心平移
    let (sin_t, cos_t) = theta_start.sin_cos();
    let mut out: Vec<(f64, f64)> = arc
        .iter()
        .map(|(px, py)| {
            (
                ux + radius * (cos_t * px - dir * sin_t * py),
                uy + radius * (sin_t * px + dir * cos_t * py),
            )
        })
        .collect();
    // 端点钉回（消浮点误差）
    out[0] = pts[0];
    let last = out.len() - 1;
    out[last] = pts[2];
    Some(out)
}

/// 单段转换（PERFECT 退化规则：点数≠3 → bezier；共线 → linear）
fn segment_anchors(seg: &Segment) -> Vec<(f64, f64)> {
    match seg.kind {
        SegKind::Linear => linear_anchors(&seg.points),
        SegKind::Catmull => catmull_anchors(&seg.points),
        SegKind::Perfect => {
            if seg.points.len() == 3 {
                if is_collinear(seg.points[0], seg.points[1], seg.points[2]) {
                    linear_anchors(&seg.points)
                } else {
                    circle_anchors(&seg.points).unwrap_or_else(|| seg.points.clone())
                }
            } else {
                seg.points.clone()
            }
        }
        SegKind::Bezier => seg.points.clone(),
    }
}

fn fmt_anchor(p: (f64, f64)) -> String {
    format!("{}:{}", p.0.round() as i64, p.1.round() as i64)
}

/// 曲线字段降级。返回 Some(新曲线) 表示有改动；None 表示原样。
/// - 单段：仅取整坐标（stable 按整数解析控制点，ppy/osu#35316）
/// - 多段：全部段转 bezier 锚点、段边界 double-up、单 B| 前缀
pub fn downgrade_curve(curve: &str) -> Option<String> {
    let (segments, dropped_trailing_type) = parse_segments(curve)?;
    if segments.len() == 1 {
        // 单段：仅取整
        let seg = &segments[0];
        let rounded: Vec<(f64, f64)> = seg
            .points
            .iter()
            .map(|(x, y)| (x.round(), y.round()))
            .collect();
        if rounded == seg.points && !dropped_trailing_type {
            return None;
        }
        let prefix = match seg.kind {
            SegKind::Bezier => "B",
            SegKind::Linear => "L",
            SegKind::Perfect => "P",
            SegKind::Catmull => "C",
        };
        let body: Vec<String> = rounded.iter().map(|p| fmt_anchor(*p)).collect();
        return Some(format!("{prefix}|{}", body.join("|")));
    }
    let mut anchors: Vec<(f64, f64)> = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let seg_anchors = segment_anchors(seg);
        if i == 0 {
            anchors.extend(seg_anchors);
        } else {
            // 段边界 double-up：重复前段末点（= 本段首点）
            if let Some(last) = anchors.last() {
                anchors.push(*last);
            }
            // 本段锚点首点 == 前段末点，跳过避免三连
            for p in seg_anchors.into_iter().skip(1) {
                anchors.push(p);
            }
        }
    }
    if anchors.len() < 2 {
        return None;
    }
    let body: Vec<String> = anchors.iter().map(|p| fmt_anchor(*p)).collect();
    Some(format!("B|{}", body.join("|")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_segment_bezier_rounded_only() {
        // 单段 bezier：只取整，不改类型
        assert_eq!(
            downgrade_curve("B|100.4:200.6|300:400"),
            Some("B|100:201|300:400".into())
        );
        assert_eq!(downgrade_curve("B|100:200|300:400"), None);
    }

    #[test]
    fn trailing_type_token_dropped() {
        // 末尾裸类型（#24570）：丢弃且单段取整
        assert_eq!(
            downgrade_curve("B|100:200|300:400|L"),
            Some("B|100:200|300:400".into())
        );
    }

    #[test]
    fn multi_segment_linear_perfect_to_single_bezier() {
        // P| 三点圆弧 + L| 折线 → 单 B|；L 段产生 double-up
        let out = downgrade_curve("P|100:100|200:200|300:100|L|400:100").unwrap();
        assert!(out.starts_with("B|"), "{out}");
        // 段边界 double-up：L 段起点重复出现
        assert!(out.contains("300:100|300:100"), "{out}");
    }

    #[test]
    fn perfect_collinear_degenerates_to_linear() {
        // 三点共线 → linear 语义（重复点编码）
        let out = downgrade_curve("P|100:100|200:100|300:100|B|500:500|600:600").unwrap();
        assert!(out.starts_with("B|"), "{out}");
        assert!(out.contains("200:100|200:100"), "{out}");
    }

    #[test]
    fn perfect_two_points_treated_as_bezier() {
        // 点数≠3 的 PERFECT → bezier 原样 + 后续段
        let out = downgrade_curve("P|100:100|200:200|L|300:300|400:400").unwrap();
        assert!(out.starts_with("B|"), "{out}");
        assert!(out.contains("100:100"), "{out}");
    }

    #[test]
    fn coordinates_clamped_and_rounded() {
        let out = downgrade_curve("B|200000.7:-5.5|B|300:300|L|400:400").unwrap();
        // clamp ±131072 后取整
        assert!(
            out.contains("131072:0") || out.contains("131072:-6") || out.contains("131072:-5"),
            "{out}"
        );
    }

    #[test]
    fn bezier_implicit_segments_split_at_duplicates() {
        // 段内重复点 = 隐式边界 → 拆段后每子段转 bezier，边界 double-up
        let out = downgrade_curve("B|100:100|200:200|200:200|300:300").unwrap();
        // 重复点保留（stable 语义需要），仍是单 B|
        assert!(out.starts_with("B|"), "{out}");
        assert!(out.contains("200:200|200:200"), "{out}");
    }

    #[test]
    fn circle_arc_endpoints_pinned() {
        // 单段 PERFECT 不转换（多段才转）——构造多段里的圆弧：
        // 圆弧段终点应钉回原始端点 (300,100)
        let out = downgrade_curve("P|100:100|200:200|300:100|L|400:100").unwrap();
        // 圆弧终点 300:100 后接 double-up
        assert!(out.contains("300:100|300:100"), "{out}");
    }

    #[test]
    fn invalid_parse_returns_none() {
        assert_eq!(downgrade_curve("B|abc:def"), None);
        assert_eq!(downgrade_curve(""), None);
    }
}
