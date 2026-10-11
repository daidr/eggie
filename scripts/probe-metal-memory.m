// Standalone Metal control: no Eggie, AppKit, shaders, fonts, or window surfaces.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <libproc.h>
#include <unistd.h>

static void sample(const char *stage, id<MTLDevice> device) {
    struct rusage_info_v4 usage = {0};
    if (proc_pid_rusage(getpid(), RUSAGE_INFO_V4, (rusage_info_t *)&usage)) {
        perror("proc_pid_rusage");
        exit(1);
    }
    printf("%s,%.3f,%.3f,%.3f\n", stage,
           usage.ri_phys_footprint / 1048576.,
           usage.ri_lifetime_max_phys_footprint / 1048576.,
           device.currentAllocatedSize / 1048576.);
    fflush(stdout);
}

int main(int argc, const char **argv) {
    NSString *mode = argc > 1 ? @(argv[1]) : @"clear";
    char *size_end = NULL;
    NSUInteger size = argc > 2 ? strtoul(argv[2], &size_end, 10) : 16;
    if (argc > 3 || (![@"clear" isEqual:mode] && ![@"empty" isEqual:mode] &&
         ![@"blit" isEqual:mode]) || size == 0 || size > 4096 ||
        (size_end && *size_end)) {
        fprintf(stderr, "Usage: probe-metal-memory [clear|empty|blit] [size: 1..4096]\n");
        return 1;
    }
    @autoreleasepool {
        printf("stage,footprint_mib,lifetime_peak_mib,metal_allocated_mib\n");
        sample("start", nil);
        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        if (!device) return 1;
        sample("device", device);
        id<MTLCommandQueue> queue = [device newCommandQueue];
        MTLTextureDescriptor *descriptor = [MTLTextureDescriptor
            texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
            width:size height:size mipmapped:NO];
        descriptor.usage = MTLTextureUsageRenderTarget;
        descriptor.storageMode = MTLStorageModePrivate;
        id<MTLTexture> texture = [device newTextureWithDescriptor:descriptor];
        if (!queue || !texture) return 1;
        sample("resources", device);
        for (int frame = 0; frame < 180; ++frame) {
            @autoreleasepool {
                if (frame % 60 == 0) {
                    id<MTLCommandBuffer> command = [queue commandBuffer];
                    if ([mode isEqual:@"clear"]) {
                        MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
                        pass.colorAttachments[0].texture = texture;
                        pass.colorAttachments[0].loadAction = MTLLoadActionClear;
                        pass.colorAttachments[0].storeAction = MTLStoreActionStore;
                        pass.colorAttachments[0].clearColor = MTLClearColorMake(.5, .5, .5, 1);
                        id<MTLRenderCommandEncoder> encoder = [command renderCommandEncoderWithDescriptor:pass];
                        [encoder endEncoding];
                    } else if ([mode isEqual:@"blit"]) {
                        id<MTLBlitCommandEncoder> encoder = [command blitCommandEncoder];
                        [encoder endEncoding];
                    }
                    sample("encoded", device);
                    [command commit];
                    [command waitUntilCompleted];
                    if (command.status == MTLCommandBufferStatusError) {
                        fprintf(stderr, "%s\n", command.error.description.UTF8String);
                        return 1;
                    }
                    sample("completed", device);
                }
                if (frame % 10 == 0) sample("idle", device);
                usleep(16667);
            }
        }
    }
    return 0;
}
