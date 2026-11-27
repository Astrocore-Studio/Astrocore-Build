import bevy_ui_render::ui_vertex_output::UiVertexOutput;

// Два круга: внутри меньшего ничего, за большим — цвет с прозрачностью `outer_alpha`, между
// ними — переход. Радиусы растут к своим наибольшим по `time` (пульсация). Перенос шейдера
// `gradient_circle_mask.swsl` SS14 @ 2f4a2aad.
struct GradientCircle {
    // RGB и прозрачность за большим кругом.
    color: vec4<f32>,
    // time, outer_radius, outer_max_radius, inner_radius.
    a: vec4<f32>,
    // inner_max_radius.
    b: vec4<f32>,
};

@group(1) @binding(0) var<uniform> circle: GradientCircle;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let distance = length((in.uv - vec2<f32>(0.5, 0.5)) * in.size);
    let time = circle.a.x;
    // Радиусы заданы долями ширины окна (`ViewportBounds.Width`).
    let width = in.size.x;
    let inner = (circle.a.w + time * (circle.b.x - circle.a.w)) * width;
    let outer = (circle.a.y + time * (circle.a.z - circle.a.y)) * width;
    let alpha_outer = circle.color.a;
    if distance > outer {
        return vec4<f32>(circle.color.rgb, alpha_outer);
    }
    if distance < inner {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    let ratio = (distance - inner) / max(outer - inner, 0.0001);
    return vec4<f32>(circle.color.rgb, alpha_outer * ratio);
}
