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
sampler2D                 diffuseMap: register( s0 );
const float4              p3dLight_ObjectSpaceLightBounds: register( c0 );
const float4              p3dNaN: register( c1 );
const float3              addcolour: register( c2 );
const float               p3dBuiltIn_Zero: register( c3 );
const float               p3dBuiltIn_One: register( c4 );
const float               addcolourfactor: register( c5 );

//
// Internal Function Implementations
//

float4 __normalize4( float4 v )
{
    return float4( normalize( v.xyz), v.w );
}

//
// Function Implementations
//
void hlslFunc_Pure3D_Colour00( const float3 diffuse,
                               const float alpha,
                               out float4 p3dPixelOutput_Colour0 )
{
    p3dPixelOutput_Colour0.rgb = diffuse;
    p3dPixelOutput_Colour0.a   = alpha;
}

void hlslFunc_Mix2( const float3 x,
                    const float3 y,
                    const float3 a,
                    out float3 output )
{
    output = lerp( x, y, a );
}

void hlslFunc_Multiply3( const float4 arg1,
                         const float4 arg2,
                         out float4 output )
{
    output = arg1 * arg2;
}

//
// Shader Inputs
//
struct PS_INPUT
{
    float2 TexCoord_0 : TEXCOORD0;
    float4 Colour_0   : COLOR0;
};

//
// Shader Outputs
//
struct PS_OUTPUT
{
    float4 p3dPixelOutput_Colour0  : COLOR0;
};

//
// Entry Point
//
void EntryPoint_ps( const PS_INPUT __psIn, out PS_OUTPUT __psOut )
{
    __psOut.p3dPixelOutput_Colour0 = float4( 0, 0, 0, 0 );

    float2 out3;
    out3 = float2( __psIn.TexCoord_0.x, __psIn.TexCoord_0.y );

    float4 out31;
    out31 = float4( __psIn.Colour_0.x, __psIn.Colour_0.y, __psIn.Colour_0.z, __psIn.Colour_0.w );

    float4 out931;
    out931 = tex2D( diffuseMap, out3 * 1 + 0 );

    float4 out942;
    hlslFunc_Multiply3( out31, out931, out942 );

    float3 out934;
    hlslFunc_Mix2( out942.xyz, addcolour, addcolourfactor, out934 );

    float4 out939;
    hlslFunc_Pure3D_Colour00( out934, out942.w, out939 );
    __psOut.p3dPixelOutput_Colour0 = out939;
}
