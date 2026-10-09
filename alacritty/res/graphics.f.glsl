#ifdef GLES2_RENDERER
precision mediump float;
varying mediump vec2 uv;
#define SAMPLE texture2D
#define OUTPUT gl_FragColor
#else
in vec2 uv;
out vec4 color;
#define SAMPLE texture
#define OUTPUT color
#endif
uniform sampler2D image;
void main() { OUTPUT = SAMPLE(image, uv); }
