#ifdef GLES2_RENDERER
attribute vec4 vertex;
varying mediump vec2 uv;
#else
in vec4 vertex;
out vec2 uv;
#endif
uniform vec2 viewport;
void main() {
    gl_Position = vec4(vertex.x * 2.0 / viewport.x - 1.0,
                       1.0 - vertex.y * 2.0 / viewport.y, 0.0, 1.0);
    uv = vertex.zw;
}
