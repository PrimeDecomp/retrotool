//! Ordered semantic IDs from MPR feature definitions.
//! GetFeatureType / GetSemanticIdNames vtable methods were recovered from
//! remastered; addresses below identify the name-list getters. These names are
//! part of the engine ABI, not a variable-length list in the archive.

/// None means an unknown definition; Some(&[]) is a known flag-only feature.
pub fn semantic_names(definition: &str) -> Option<&'static [&'static str]> {
    Some(match definition {
        // 0x7100147fb8
        "AERO" => &[],
        // 0x7100169da0
        "AMBS" => &["AmbientSource"],
        // 0x7100169658
        "ANUV" => &["TexCoordTransform", "TexCoordTransU"],
        // 0x7100146630
        "AOSM" => &[],
        // 0x710016c87c
        "BLPB" => &["Value"],
        // 0x710014621c
        "BMSM" => &[],
        // 0x7100178004
        "BPTX" => &[
            "ViewToTextureTransform",
            "ViewToProbeSpaceInverseTranspose",
            "Texture_Ambient",
            "Texture_MidpointRGB",
            "Texture_RangeRGB",
            "Texture_ExponentRGB",
            "Texture_GradientDirR",
            "Texture_GradientDirG",
            "Texture_GradientDirB",
        ],
        // 0x7100146ae8
        "BRSM" => &[],
        // 0x710014a440
        "CACM" => &[],
        // 0x7100169160
        "CAUS" => &["Data", "Texture"],
        // 0x710016a974
        "CCLL" => &[
            "Params",
            "LightSpheres0",
            "LightSpheres1",
            "CullPlanesX",
            "CullPlanesY",
            "CullPlanesZ",
            "ClusterMask",
        ],
        // 0x7100144a8c
        "CIDF" => &[],
        // 0x710014a26c
        "CIMO" => &[],
        // 0x71001448b0
        "CLDF" => &[],
        // 0x7100171de0
        "CLPP" => &[
            "ClusterSizeParams",
            "ClusterDepthParams",
            "ClusterIndexParams",
            "ClusterLightMask",
            "ClusterShadowMapLightMask",
            "UnshadowedLights",
            "SpotShadowMapLights",
            "SpotShadowMapData",
            "SpotSharedShadowMapTexture?",
            "PointShadowMapLights",
            "PointShadowMapData",
            "PointSharedShadowMapTexture?",
        ],
        // 0x7100145e30
        "CRSM" => &[],
        // 0x7100146c30
        "CTSM" => &[],
        // 0x710016410c
        "DATA" => &[],
        // 0x7100171740
        "DCAL" => &["DecalInstanceData"],
        // 0x7100174e24
        "DDLT" => &[
            "CountParams",
            "Lights",
            "ShadowMapLights",
            "ShadowMapData",
            "ShadowMapDitherTexture?",
            "ShadowMapDitherTextureSizeParams?",
            "SharedShadowMapTexture?",
        ],
        // 0x7100163b3c
        "DEFX" => &["ColorDensityMap", "DepthMap", "DepthDensityRamp", "FogParams"],
        // 0x7100148ae0
        "DMOD" => &[],
        // 0x7100144bcc
        "DVSM" => &[],
        // 0x7100144ea0
        "DVTM" => &[],
        // 0x7100b3e258
        "ENFM" => &[],
        // 0x7100147848
        "FDEP" => &[],
        // 0x71001470d8
        "FFTX" => &[],
        // 0x7100147490
        "FHIG" => &[],
        // 0x71001472b4
        "FHOL" => &[],
        // 0x7100163334
        "FLT1" => &["Value"],
        // 0x7100163724
        "FLT4" => &["Value"],
        // 0x710014766c
        "FMUL" => &[],
        // 0x7100165a84
        "FOGS" => &["DepthDensityRamp", "ColorRamp", "FogParams", "ColorRampParams"],
        // 0x7100167ccc
        "FOGV" => &["DensityScale", "DensityOffset", "DensityTexture", "ViewToModel"],
        // 0x7100145738
        "FPSM" => &[],
        // 0x7100147a24
        "FSDF" => &[],
        // 0x71001460d4
        "FXSM" => &[],
        // 0x7100b3e584
        "GMSM" => &[],
        // 0x7100171ac4
        "GSV_" => &["Values"],
        // 0x7100b3e43c
        "HETM" => &[],
        // 0x7100170a44
        "HFUD" => &["HeightfieldData"],
        // 0x71001628b8
        "ID32" => &[],
        // 0x7100162b58
        "INT1" => &["Value"],
        // 0x7100162f44
        "INT4" => &["Value"],
        // 0x710016f6cc
        "LAYR" => &[
            "Texture0",
            "Texture1",
            "BlendTexture",
            "TexCoordIndex0",
            "TexCoordIndex1",
            "BlendTexCoordIndex",
            "Texture0SizeParams",
            "Texture1SizeParams",
            "BlendTextureSizeParams",
        ],
        // 0x710016b110
        "LIT_" => &["LightData"],
        // 0x7100148c28
        "LSHM" => &[],
        // 0x71001468f4
        "LSSM" => &[],
        // 0x7100149cb0
        "LUSM" => &[],
        // 0x7100165694
        "MAT4" => &["Value"],
        // 0x7100141a84
        "MF1B" => &[],
        // 0x7100141bdc
        "MF1E" => &[],
        // 0x7100142544
        "MFAE" => &[],
        // 0x7100141d34
        "MFAV" => &[],
        // 0x7100142d58
        "MFBL" => &[],
        // 0x7100142c00
        "MFBP" => &[],
        // 0x7100143008
        "MFC0" => &[],
        // 0x7100143160
        "MFC1" => &[],
        // 0x71001432b8
        "MFC2" => &[],
        // 0x7100143410
        "MFC3" => &[],
        // 0x7100143568
        "MFC4" => &[],
        // 0x7100141fe4
        "MFCA" => &[],
        // 0x71001427f8
        "MFCI" => &[],
        // 0x710014213c
        "MFIM" => &[],
        // 0x7100141e8c
        "MFIN" => &[],
        // 0x7100142eb0
        "MFLB" => &[],
        // 0x7100141524
        "MFMT" => &[],
        // 0x7100142950
        "MFOI" => &[],
        // 0x710014269c
        "MFOT" => &[],
        // 0x71001423ec
        "MFRL" => &[],
        // 0x71001417d4
        "MFSK" => &[],
        // 0x710014167c
        "MFSR" => &[],
        // 0x710013fbe4
        "MFTR" => &[],
        // 0x7100142aa8
        "MFVA" => &[],
        // 0x710014192c
        "MFVC" => &[],
        // 0x710014923c
        "MSSM" => &[],
        // 0x710016f220
        "MTEX" => &["Value"],
        // 0x7100142294
        "MTSM" => &[],
        // 0x7100147c00
        "PFNM" => &[],
        // 0x7100145c28
        "PLSM" => &[],
        // 0x7100149abc
        "PMOD" => &[],
        // 0x71001619ec
        "POD_" => &["Value"],
        // 0x7100147ddc
        "PROT" => &[],
        // 0x71001496d4
        "PTSM" => &[],
        // 0x7100171040
        "REFL" => &["BRDF", "Params"],
        // 0x7100148e30
        "RPSM" => &[],
        // 0x710016c87c
        "SBUF" => &["Value"],
        // 0x7100144fe8
        "SCSM" => &[],
        // 0x710014a088
        "SDMD" => &[],
        // 0x7100145544
        "SDSM" => &[],
        // 0x710016c1b4
        "SH9L" => &[
            "OverflowCoeff_X_Y_Z_C_R",
            "OverflowCoeff_X_Y_Z_C_G",
            "OverflowCoeff_X_Y_Z_C_B",
            "OverflowCoeff_XY_XZ_YZ_Z2_R",
            "OverflowCoeff_XY_XZ_YZ_Z2_G",
            "OverflowCoeff_XY_XZ_YZ_Z2_B",
            "OverflowCoeff_X2Y2_RGB",
        ],
        // 0x710016d12c
        "SHDW" => &[
            "ViewToGoboMap",
            "GoboMapTexture",
            "GoboMapRadialTransform",
            "ViewToShadowMap",
            "ShadowMapTexture",
            "ShadowColor",
            "ViewToLight",
            "ShadowMapProjParams",
            "ShadowDecayRange",
            "ShadowCascadeRanges",
            "ViewToCascadeShadowMap0",
            "ViewToCascadeShadowMap1",
            "ViewToCascadeShadowMap2",
            "ViewToCascadeShadowMap3",
            "ShadowMapTextureSizeParams",
        ],
        // 0x71001662f4
        "SKIN" => &["BoneData"],
        // 0x71001490f4
        "SLSM" => &[],
        // 0x7100149e94
        "SODG" => &[],
        // 0x7100146ef4
        "SOTM" => &[],
        // 0x7100145ae0
        "SRSM" => &[],
        // 0x71001732cc
        "SSCB" => &["PositionBounds", "TexCoordBounds"],
        // 0x71001770a0
        "SSPR" => &[
            "ShadowBoundsMin",
            "ShadowBoundsMax",
            "OpacityRangeParams",
            "OpacityAttenuationParams",
            "SizeScaleRangeParams",
            "SizeScaleAttenuationParams",
        ],
        // 0x710014590c
        "SSSM" => &[],
        // 0x7100145210
        "STSM" => &[],
        // 0x710016825c
        "TERR" => &["TerrainData"],
        // 0x710016442c
        "TEXS" => &["Texture", "TexCoordIndex", "TextureSizeParams"],
        // 0x71001464f0
        "TFDV" => &[],
        // 0x7100148194
        "TFIM" => &[],
        // 0x7100148370
        "TFSP" => &[],
        // 0x71001766fc
        "VFDR" => &[
            "Params",
            "DensityMap0",
            "DensityMap1",
            "DensityMap2",
            "DensityMap3",
            "DensityMap4",
            "DensityMap5",
            "DensityMap6",
            "DensityMap7",
        ],
        // 0x710016cc38
        "VFOG" => &["FogValues", "FogParams"],
        // 0x7100149500
        "VFSM" => &[],
        // 0x71001776a4
        "VMCD" => &["CascadeCount", "CascadeViewportScale", "CascadeViewportTranslate"],
        // 0x710014854c
        "WBOT" => &[],
        // 0x7100148904
        "WBPT" => &[],
        // 0x71001678d0
        "WIND" => &["WindAnimData"],
        // 0x7100148728
        "WLIT" => &[],
        // 0x71001498c8
        "WSDM" => &[],
        // 0x7100171434
        "XFRM" => &["GlobalTransforms"],
        _ => return None,
    })
}
