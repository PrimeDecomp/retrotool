use binrw::binrw;

#[binrw]
#[derive(Clone, Debug)]
pub struct CVector3f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CAABox {
    pub min: CVector3f,
    pub max: CVector3f,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CTransform4f {
    m00: f32,
    m01: f32,
    m02: f32,
    m03: f32,
    m10: f32,
    m11: f32,
    m12: f32,
    m13: f32,
    m20: f32,
    m21: f32,
    m22: f32,
    m23: f32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct COBBox {
    xf : CTransform4f,
    extents: CVector3f,
}
