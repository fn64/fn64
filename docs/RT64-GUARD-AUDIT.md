# RT64 guard audit — WORK IN PROGRESS

Confirmed so far:
- TextureLutModeError::ReservedEncoding — INVENTED (angrylion rdp.c:630-631 = two 1-bit fields)
- TexrectExecutionError::ReservedAlphaCompare — INVENTED (angrylion rdp.c:659-660, blender.c:75)
- FillCoordinateError::FractionalEdge — WRONG RESPONSE (RT64 rt64_rdp.cpp:1043-1047 rounds)
- TexrectExecutionError::OutsideTarget — WRONG RESPONSE (angrylion rasterizer.c:2349-2363 clamps)
