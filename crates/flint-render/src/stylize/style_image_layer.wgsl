// One layer of a stack of style images (#547, stylize/images.rs's
// LayerInfo): what was measured of the image, and its size, since the
// layers of one stack differ in size (each sits in its layer's top-left
// corner). Its own file because a style's uniform holds an array of them
// in every pass, and only the drawing pass reads the images
// (style_image.wgsl).
struct StyleImageLayer {
    // mean and spread of luma (a paper's), ink (a stroke strip's), unused
    stats: vec4<f32>,
    // level-0 width and height, mip levels, unused
    size: vec4<u32>,
};
