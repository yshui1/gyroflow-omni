// SPDX-License-Identifier: GPL-3.0-or-later
// Standalone QRhi/D3D11 correctness and timing check, using the shipped LUT shader.
#include <QGuiApplication>
#include <QFile>
#include <QElapsedTimer>
#include <qfloat16.h>
#include <rhi/qrhi.h>
#include <rhi/qrhi_platform.h>
#include <private/qshader_p.h>
#include <memory>
#include <vector>
#include <algorithm>
#include <cstdio>

static QShader shader(const QString &path) {
    QFile f(path);
    if (!f.open(QIODevice::ReadOnly)) qFatal("Cannot read shader");
    return QShader::fromSerialized(f.readAll());
}

int main(int argc, char **argv) {
    setvbuf(stdout, nullptr, _IONBF, 0);
    QGuiApplication app(argc, argv);
    if (argc != 4) qFatal("Usage: preview-lut-test vertex.qsb lut.frag.qsb baseline.frag.qsb");
    QRhiD3D11InitParams init;
    std::unique_ptr<QRhi> rhi(QRhi::create(QRhi::D3D11, &init, QRhi::EnableTimestamps));
    if (!rhi) qFatal("D3D11 initialization failed");
    std::printf("GPU: %s\n", qPrintable(rhi->driverInfo().deviceName));
    const QSize size(3840, 2160);
    const int n = 33, tiles = 6, edge = n * tiles;
    std::vector<qfloat16> lut(edge * edge * 4);
    for (int b = 0; b < n; ++b) for (int g = 0; g < n; ++g) for (int r = 0; r < n; ++r) {
        int p = ((b / tiles * n + g) * edge + b % tiles * n + r) * 4;
        lut[p] = qfloat16(float(b) / (n - 1));
        lut[p+1] = qfloat16(1.0f - float(r) / (n - 1));
        lut[p+2] = qfloat16(float(g) / (n - 1)); lut[p+3] = qfloat16(1.0f);
    }
    QByteArray input(size.width() * size.height() * 4, '\0');
    for (int y = 0; y < size.height(); ++y) for (int x = 0; x < size.width(); ++x) {
        auto p = reinterpret_cast<unsigned char *>(input.data()) + (y * size.width() + x) * 4;
        p[0] = (x * 13 + y * 7) % 256; p[1] = (x + y * 3) % 256;
        p[2] = (x * 3 + y) % 256; p[3] = 255;
    }
    auto texture = [&](QRhiTexture::Format fmt, QSize sz, QRhiTexture::Flags flags = {}) {
        std::unique_ptr<QRhiTexture> t(rhi->newTexture(fmt, sz, 1, flags));
        if (!t->create()) qFatal("Texture creation failed"); return t;
    };
    auto source = texture(QRhiTexture::RGBA8, size);
    auto output = texture(QRhiTexture::RGBA8, size, QRhiTexture::RenderTarget | QRhiTexture::UsedAsTransferSource);
    auto atlas = texture(QRhiTexture::RGBA16F, QSize(edge, edge));
    auto matrices = texture(QRhiTexture::R32F, QSize(16, 1));
    auto canvas = texture(QRhiTexture::R8, QSize(1, 1));
    auto mesh = texture(QRhiTexture::R32F, QSize(1, 2048));
    std::unique_ptr<QRhiTextureRenderTarget> rt(rhi->newTextureRenderTarget({QRhiColorAttachment(output.get())}));
    std::unique_ptr<QRhiRenderPassDescriptor> rp(rt->newCompatibleRenderPassDescriptor());
    rt->setRenderPassDescriptor(rp.get()); if (!rt->create()) qFatal("Render target creation failed");
    auto buffer = [&](int bytes, QRhiBuffer::UsageFlags usage) {
        std::unique_ptr<QRhiBuffer> b(rhi->newBuffer(QRhiBuffer::Dynamic, usage, bytes));
        if (!b->create()) qFatal("Buffer creation failed"); return b;
    };
    auto transform = buffer(80, QRhiBuffer::UniformBuffer);
    auto params = buffer(512, QRhiBuffer::UniformBuffer);
    auto uniforms = buffer(48, QRhiBuffer::UniformBuffer);
    const float vertices[] = {-1,1,0,0, -1,-1,0,1, 1,1,1,0, 1,-1,1,1};
    auto vb = buffer(sizeof(vertices), QRhiBuffer::VertexBuffer);
    std::unique_ptr<QRhiSampler> sampler(rhi->newSampler(QRhiSampler::Linear, QRhiSampler::Linear, QRhiSampler::None, QRhiSampler::ClampToEdge, QRhiSampler::ClampToEdge));
    if (!sampler->create()) qFatal("Sampler creation failed");
    std::unique_ptr<QRhiShaderResourceBindings> srb(rhi->newShaderResourceBindings());
    auto stages = QRhiShaderResourceBinding::VertexStage | QRhiShaderResourceBinding::FragmentStage;
    srb->setBindings({
        QRhiShaderResourceBinding::uniformBuffer(0, stages, transform.get()),
        QRhiShaderResourceBinding::sampledTexture(1, QRhiShaderResourceBinding::FragmentStage, source.get(), sampler.get()),
        QRhiShaderResourceBinding::uniformBuffer(2, QRhiShaderResourceBinding::FragmentStage, params.get()),
        QRhiShaderResourceBinding::sampledTexture(3, QRhiShaderResourceBinding::FragmentStage, matrices.get(), sampler.get()),
        QRhiShaderResourceBinding::sampledTexture(4, QRhiShaderResourceBinding::FragmentStage, canvas.get(), sampler.get()),
        QRhiShaderResourceBinding::sampledTexture(5, QRhiShaderResourceBinding::FragmentStage, mesh.get(), sampler.get()),
        QRhiShaderResourceBinding::sampledTexture(6, QRhiShaderResourceBinding::FragmentStage, atlas.get(), sampler.get()),
        QRhiShaderResourceBinding::uniformBuffer(7, QRhiShaderResourceBinding::FragmentStage, uniforms.get())
    }); if (!srb->create()) qFatal("Resource bindings creation failed");
    auto pipeline = [&](const QString &path) {
        std::unique_ptr<QRhiGraphicsPipeline> p(rhi->newGraphicsPipeline());
        p->setShaderStages({{QRhiShaderStage::Vertex, shader(argv[1])}, {QRhiShaderStage::Fragment, shader(path)}});
        QRhiVertexInputLayout layout; layout.setBindings({{4 * sizeof(float)}});
        layout.setAttributes({{0,0,QRhiVertexInputAttribute::Float2,0}, {0,1,QRhiVertexInputAttribute::Float2,2*sizeof(float)}});
        p->setVertexInputLayout(layout); p->setTopology(QRhiGraphicsPipeline::TriangleStrip);
        p->setShaderResourceBindings(srb.get()); p->setRenderPassDescriptor(rp.get());
        if (!p->create()) qFatal("Pipeline creation failed"); return p;
    };
    auto withLut = pipeline(argv[2]); auto baseline = pipeline(argv[3]);
    auto updates = rhi->nextResourceUpdateBatch();
    float matrix[20] = {}; matrix[0] = matrix[5] = matrix[10] = matrix[15] = 1;
    updates->updateDynamicBuffer(transform.get(), 0, sizeof(matrix), matrix);
    updates->updateDynamicBuffer(vb.get(), 0, sizeof(vertices), vertices);
    QByteArray zeros(512, '\0'); updates->updateDynamicBuffer(params.get(), 0, zeros.size(), zeros.data());
    float domain[12] = {0,0,0,float(n), 1,1,1,float(tiles), 1,0,0,0};
    updates->updateDynamicBuffer(uniforms.get(), 0, sizeof(domain), domain);
    updates->uploadTexture(source.get(), QRhiTextureUploadDescription({QRhiTextureUploadEntry(0,0,QRhiTextureSubresourceUploadDescription(input.constData(), input.size()))}));
    updates->uploadTexture(atlas.get(), QRhiTextureUploadDescription({QRhiTextureUploadEntry(0,0,QRhiTextureSubresourceUploadDescription(lut.data(), lut.size()*sizeof(qfloat16)))}));
    auto render = [&](QRhiGraphicsPipeline *p, bool readback = false) {
        QRhiCommandBuffer *cb = nullptr;
        if (rhi->beginOffscreenFrame(&cb) != QRhi::FrameOpSuccess) qFatal("Frame begin failed");
        cb->beginPass(rt.get(), Qt::black, {1,0}, updates); updates = nullptr;
        cb->setGraphicsPipeline(p); cb->setViewport({0,0,float(size.width()),float(size.height())});
        cb->setShaderResources(); QRhiCommandBuffer::VertexInput binding(vb.get(),0);
        cb->setVertexInput(0,1,&binding); cb->draw(4); cb->endPass();
        QRhiReadbackResult result;
        if (readback) { auto u = rhi->nextResourceUpdateBatch(); u->readBackTexture({output.get()}, &result); cb->resourceUpdate(u); }
        rhi->endOffscreenFrame(); rhi->finish();
        if (readback) {
            int maxError = 0;
            for (int i = 0; i < input.size(); i += 4) {
                auto src = reinterpret_cast<const unsigned char *>(input.constData()) + i;
                auto dst = reinterpret_cast<const unsigned char *>(result.data.constData()) + i;
                maxError = std::max({maxError, std::abs(int(dst[0])-src[2]), std::abs(int(dst[1])-(255-src[0])), std::abs(int(dst[2])-src[1]), std::abs(int(dst[3])-255)});
            }
            std::printf("4K readback maximum error: %d/255\n", maxError);
            if (maxError > 1) qFatal("GPU LUT output mismatch");
        }
        return cb->lastCompletedGpuTime() * 1000.0;
    };
    render(withLut.get(), true);
    std::vector<double> a, b;
    for (int i = 0; i < 220; ++i) {
        double x, y;
        if (i % 2) { y = render(withLut.get()); x = render(baseline.get()); }
        else { x = render(baseline.get()); y = render(withLut.get()); }
        if (i >= 20) { a.push_back(x); b.push_back(y); }
    }
    std::sort(a.begin(), a.end()); std::sort(b.begin(), b.end());
    std::printf("4K GPU median (200 frames): baseline %.4f ms, LUT %.4f ms, overhead %.4f ms\n", a[100], b[100], b[100]-a[100]);
    return 0;
}
