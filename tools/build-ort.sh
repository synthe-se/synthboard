#!/bin/zsh
# Builds ONNX Runtime 1.31.1 as one static archive with cmake and ninja, without build.py. Python still
# runs: CMake's probes, and the helpers of its configure step. Only gen_proto.py is replaced, below.
# usage: build-ort.sh <android|mac> <Release|MinSizeRel> [jobs]
# It needs a seed tree, a finished earlier build at $seed, and fails without one. That tree gives the
# fetched dependencies, the protoc binary, the protobuf outputs, and `op_reduction.generated`, the kernels
# reduced to the model's by ONNX Runtime's own Python tool. Nothing here makes those from nothing.
# A new flag only takes effect in a new build directory: remove $build first.
set -eu
platform=$1; config=$2; jobs=${3:-6}
root=${0:A:h:h}
infer=$root/data/raw/ort
src=$infer/ortsrc
seed=$infer/ortbuild/min_ext/MinSizeRel     # earlier Android tree: deps and generated sources
ndk=$HOME/Library/Android/sdk/ndk/30.0.16248370
build=$infer/ortbuild/bench_${platform}_$config
dist=$infer/$platform
[[ -d $seed/_deps ]] || { echo "No seed tree at $seed: see the header of this script."; exit 1; }
mkdir -p $build $dist

# The generated kernel registrations come from the Python reducer; reuse them, do not regenerate.
[[ -d $build/op_reduction.generated ]] || cp -R $seed/op_reduction.generated $build/

# Only gen_proto.py needs an interpreter at build time. Its outputs are already in the seed tree, so
# this shim copies them. CMake's FindPython probes still reach the system interpreter for a version.
shim=$build/python-shim.sh
cat > $shim <<SHIM
#!/bin/sh
for a in "\$@"; do case "\$a" in *gen_proto.py)
  while [ \$# -gt 0 ]; do [ "\$1" = "-o" ] && out=\$2; shift; done
  cp $seed/_deps/onnx-build/onnx/*.proto $seed/_deps/onnx-build/onnx/*.proto3 $seed/_deps/onnx-build/onnx/*.pb.h $seed/_deps/onnx-build/onnx/*.pb.cc "\$out"/ 2>/dev/null
  exit 0;; esac; done
exec /usr/bin/python3 "\$@"
SHIM
chmod +x $shim

deps=()
for d in $seed/_deps/*-src(/); do
  n=${${d:t}%-src}; deps+=(-DFETCHCONTENT_SOURCE_DIR_${(U)n}=$d)
done

prefix_map="-ffile-prefix-map=$infer=ort -ffile-prefix-map=$HOME=~"
common=(
  -Wno-dev -G Ninja -DCMAKE_BUILD_TYPE=$config -DPython_EXECUTABLE=$shim -DPython3_EXECUTABLE=$shim
  -DFETCHCONTENT_FULLY_DISCONNECTED=ON -DFETCHCONTENT_QUIET=ON
  -Donnxruntime_USE_TELEMETRY=OFF -Donnxruntime_USE_MIMALLOC=OFF -Donnxruntime_ENABLE_PYTHON=OFF
  -Donnxruntime_BUILD_SHARED_LIB=OFF -Donnxruntime_BUILD_UNIT_TESTS=OFF -Donnxruntime_BUILD_BENCHMARKS=OFF
  -Donnxruntime_DISABLE_ML_OPS=ON -Donnxruntime_DISABLE_RTTI=ON -Donnxruntime_DISABLE_EXCEPTIONS=ON
  -Donnxruntime_MINIMAL_BUILD=ON -Donnxruntime_EXTENDED_MINIMAL_BUILD=ON -Donnxruntime_MINIMAL_BUILD_CUSTOM_OPS=OFF
  -Donnxruntime_REDUCED_OPS_BUILD=ON -DONNX_MINIMAL_BUILD=ON -Donnxruntime_USE_VCPKG=OFF
  -Donnxruntime_DISABLE_FLOAT8_TYPES=ON -Donnxruntime_DISABLE_FLOAT4_TYPES=ON
  -DONNX_CUSTOM_PROTOC_EXECUTABLE=$seed/_deps/protoc_binary-src/bin/protoc
  -Donnxruntime_DEV_MODE=OFF
  # `__FILE__` lands in the strings of the library, and tools/check-release.sh refuses a machine path.
  "-DCMAKE_C_FLAGS=$prefix_map" "-DCMAKE_ASM_FLAGS=$prefix_map"
)
if [[ $platform == android ]]; then
  plat=(
    -DCMAKE_TOOLCHAIN_FILE=$ndk/build/cmake/android.toolchain.cmake -DANDROID_ABI=arm64-v8a
    -DANDROID_PLATFORM=android-29 -DANDROID_MIN_SDK=29 -DANDROID_STL=c++_static
    -DANDROID_USE_LEGACY_TOOLCHAIN_FILE=false -Donnxruntime_USE_SVE=ON -Donnxruntime_USE_KLEIDIAI=ON
    "-DCMAKE_CXX_FLAGS=-Wno-error=unused-const-variable $prefix_map"
  )
  ar=$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-ar
else
  # KleidiAI trips a -Werror on this host. The host build proves loading and bytes, not Android kernels.
  plat=(-DCMAKE_OSX_ARCHITECTURES=arm64 -DCMAKE_OSX_DEPLOYMENT_TARGET=14.0 -Donnxruntime_USE_KLEIDIAI=OFF
    "-DCMAKE_CXX_FLAGS=$prefix_map")
  ar=$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-ar
fi

cd $build
[[ -f build.ninja ]] || cmake $src/cmake $common $plat $deps
nice -n 10 ninja -j $jobs onnxruntime_session onnxruntime_providers onnxruntime_optimizer onnxruntime_framework \
  onnxruntime_graph onnxruntime_util onnxruntime_common onnxruntime_mlas onnxruntime_flatbuffers onnxruntime_lora \
  onnx onnx_proto cpuinfo

# ort-sys takes a single libonnxruntime.a when present (static_link/mod.rs), so merge everything built.
libs=(libonnxruntime_session.a libonnxruntime_optimizer.a libonnxruntime_providers.a libonnxruntime_framework.a
      libonnxruntime_graph.a libonnxruntime_util.a libonnxruntime_common.a libonnxruntime_mlas.a
      libonnxruntime_flatbuffers.a libonnxruntime_lora.a)
libs+=(_deps/**/*.a(.N))
{
  echo "create $dist/libonnxruntime.a"
  for l in $libs; do case $l in *gtest*|*gmock*|*libre2*|*libprotobuf.a|*protoc*) ;; *) echo "addlib $l";; esac; done
  echo save; echo end
} > $build/merge.mri
rm -f $dist/libonnxruntime.a
$ar -M < $build/merge.mri
cp $build/merge.mri $dist/
ls -la $dist/libonnxruntime.a
