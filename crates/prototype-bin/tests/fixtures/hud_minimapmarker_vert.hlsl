//-----------------------------------------------------------------------------
///
//  Copyright (c) 2005 Radical Entertainment Ltd. All Rights Reserved.
///
//-----------------------------------------------------------------------------



//
// pass = lit
//

//
// Global vars
//
const float4x4            p3dWorldViewProjectionMatrix: register( c0 );
const float4              p3dLight_ObjectSpaceLightBounds: register( c4 );
const float4              p3dNaN: register( c5 );

//
// Function Implementations
//
void hlslFunc_GenerateWVPPosition0( const float4 position,
                                    const float4x4 world_view_proj_matrix,
                                    out float4 p3dPositionWorldViewProjection )
{
    p3dPositionWorldViewProjection = mul( world_view_proj_matrix, position );
}

//
// Shader Inputs
//
struct VS_INPUT
{
    float4 p3dVertex_Position     : POSITION;
    float2 p3dVertex_Texcoord0    : TEXCOORD0;
    float4 p3dVertex_Colour0      : COLOR0;
};

//
// Shader Outputs
//
struct VS_OUTPUT
{
    float4 p3dOutputPosition       : POSITION;
    float2 TexCoord_0              : TEXCOORD0;
    float4 Colour_0                : COLOR0;
};

//
// Entry Point
//
void EntryPoint_vs( const VS_INPUT __vsIn, out VS_OUTPUT __vsOut )
{

    float4 out0;
    out0 = __vsIn.p3dVertex_Position;
#ifndef RAD_OGL
    out0.w = 1.0;
#endif //RAD_OGL

    float2 out2;
    out2 = __vsIn.p3dVertex_Texcoord0;

    float4 out30;
    out30 = __vsIn.p3dVertex_Colour0;

    float4 out237;
    hlslFunc_GenerateWVPPosition0( out0, p3dWorldViewProjectionMatrix, out237 );
    __vsOut.p3dOutputPosition = out237;

#ifdef RAD_XENON
    float4 vk_lightPos = p3dLight_ObjectSpaceLightBounds;
    vk_lightPos.w = 1.0f;

    float4 vk_position = __vsIn.p3dVertex_Position;
    vk_position.w = 1.0f;

    float vk_lightRadius = p3dLight_ObjectSpaceLightBounds.w;
    if(distance(vk_position, vk_lightPos) > (__vsIn.p3dVertex_Position.w + vk_lightRadius))
    {
        __vsOut.p3dOutputPosition = p3dNaN;
    }
#endif //RAD_XENON

    __vsOut.TexCoord_0 = float2( out2.x, out2.y );
    __vsOut.Colour_0 = float4( out30.x, out30.y, out30.z, out30.w );

}
