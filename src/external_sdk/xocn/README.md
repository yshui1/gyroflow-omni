# Sony X-OCN desktop preview

The optional Windows x64 MDK reader opens Sony FX5 X-OCN family H MXF files in place. The player displays an SDR preview at half the active raster width and height and presents the first two PCM audio channels as stereo. Gyroflow uses the original MXF for motion data, timing, and source dimensions. Desktop render and optical analysis still use the existing rendering backend, which cannot decode X-OCN.

## Build and run

Use an x64 Visual Studio developer shell with CMake, NMake and clang-cl on PATH. Use the [MDK SDK](https://github.com/wang-bin/mdk-sdk) and ABI headers from the same build. Extract the ABI package into the SDK so `include/abi/mdk/FrameReader.h` is present. The local build was tested with MDK 0.40.1 and ABI 20261112.

```powershell
cmake -S src/external_sdk/xocn -B target/mdk-xocn -G "NMake Makefiles" -DCMAKE_CXX_COMPILER=clang-cl -DCMAKE_BUILD_TYPE=Release "-DMDK_SDK=C:/path/to/mdk-sdk"
cmake --build target/mdk-xocn
```

Place `mdk-xocn.dll` beside the Gyroflow executable and matching MDK runtime. In the same folder, write the absolute path of a locally available Sony RAW Development SDK 6.0 DLL to `SonyRawDev.path` as one line of UTF-8 text. `GYROFLOW_SONY_RAW_DLL` overrides that file when set. The test used Sony RAW Development SDK 6.0.0.06160_7101. The Sony library is loaded at runtime and is outside this build. [Sony RAW SDK program](https://pro.sony/s3/2026/10/28072308/Sony_X-OCN_WhitePaper_v3.1.pdf#page=21)

The Windows adapter uses the RAW control and buffer layout documented by [Dec18studios/fx5-xocn-tools](https://github.com/Dec18studios/fx5-xocn-tools).
