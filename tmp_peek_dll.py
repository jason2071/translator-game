data = open(r'F:/Downloads/new/Rebirth_Pub_v1.1.0/Rebirth Pub_Data/Managed/UnityEngine.TextCoreFontEngineModule.dll', 'rb').read()
print('--- TextCoreFontEngineModule ---')
for needle in [b'GetFaceInfo', b'LoadFontFace', b'InitializeFontEngine', b'UnloadFontEngine']:
    print('  ', needle.decode(), data.count(needle))
