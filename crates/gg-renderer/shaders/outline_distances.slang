// The outline's first pass, a pixel a thread, before outline.slang: into an image of scalars, each
// pixel's distance along its row, within a reach, to the nearest pixel showing one of a set of IDs.
// Euclidean distance separates, so the second pass finishes it over the rows.
//
// A workgroup takes 64 pixels of 4 rows. Each pixel's ID is binary-searched in the sorted list once
// and shared through groupshared memory with the `reach` pixels either side: about
// 1 + 2 * reach / 64 searches a pixel instead of 1 + 2 * reach.
//
// `Push` matches `DistancePush` in src/view.rs field for field.

[[vk::binding(0, 0)]] Texture2D<uint2> ids[];
[[vk::binding(2, 0)]] [format("r32f")] RWTexture2DArray<float> scalars[];

// Pixels across and down a workgroup: `GROUP` in src/view.rs for both.
static const uint GROUP_X = 64;
static const uint GROUP_Y = 4;
// The largest reach: `MAX_REACH` in src/view.rs.
static const uint MAX_REACH = 17;

struct Push {
    // The IDs, sorted, as pairs of words, low half first, as draws write them.
    uint* list;
    // The view's, in pixels.
    uint2 size;
    // The view's IDs (a bindless texture) and the target (a bindless storage image).
    uint image;
    uint target;
    // IDs in the list.
    uint count;
    // How far along a row a pixel looks, in pixels, from 1 to MAX_REACH.
    uint reach;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// Whether each pixel of the workgroup's rows, and `reach` past their ends, shows one of the IDs.
groupshared uint picked[GROUP_Y][GROUP_X + 2 * MAX_REACH];

// Whether the pixel at `at` shows a draw of one of the IDs; none outside the view does.
bool is_picked(int2 at) {
    if (any(at < 0) || any(at >= int2(push.size))) {
        return false;
    }
    uint2 id = ids[push.image].Load(int3(at, 0));
    // The list's first ID no less than this one.
    uint low = 0;
    uint high = push.count;
    while (low < high) {
        uint middle = (low + high) / 2;
        uint2 entry = uint2(push.list[2 * middle], push.list[2 * middle + 1]);
        if (entry.y < id.y || (entry.y == id.y && entry.x < id.x)) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    return low < push.count && push.list[2 * low] == id.x && push.list[2 * low + 1] == id.y;
}

[shader("compute")]
[numthreads(GROUP_X, GROUP_Y, 1)]
void distances_main(uint3 pixel: SV_DispatchThreadID, uint3 local: SV_GroupThreadID) {
    int reach = int(push.reach);
    // Slot s of a row holds the pixel s - reach from the workgroup's first. A thread fills its own
    // pixel's slot and, the halo being under 64 pixels, at most one more.
    int first = int(pixel.x - local.x) - reach;
    int slot = int(local.x);
    picked[local.y][slot] = is_picked(int2(first + slot, pixel.y)) ? 1 : 0;
    slot += GROUP_X;
    if (slot < GROUP_X + 2 * reach) {
        picked[local.y][slot] = is_picked(int2(first + slot, pixel.y)) ? 1 : 0;
    }
    GroupMemoryBarrierWithGroupSync();
    if (any(pixel.xy >= push.size)) {
        return;
    }
    // Past the reach, which the second pass reads as no pixel along the row.
    float nearest = float(reach + 1);
    int middle = int(local.x) + reach;
    for (int away = 0; away <= reach; away++) {
        if (picked[local.y][middle - away] != 0 || picked[local.y][middle + away] != 0) {
            nearest = float(away);
            break;
        }
    }
    scalars[push.target][uint3(pixel.xy, 0)] = nearest;
}
