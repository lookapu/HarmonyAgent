from PIL import Image, ImageChops, ImageStat
import os

before = r"C:\Users\<USER>\AppData\Local\Temp\qoder-computer-use-images\7c175deb\img-1786716807454625800-045798.png"
after  = r"C:\Users\<USER>\AppData\Local\Temp\qoder-computer-use-images\7c175deb\img-1786716845705658700-266239.png"

for name, p in [("before", before), ("after", after)]:
    print(name, "exists:", os.path.exists(p), os.path.getsize(p) if os.path.exists(p) else 0)

im1 = Image.open(before).convert("RGB")
im2 = Image.open(after).convert("RGB")
print("sizes:", im1.size, im2.size)

diff = ImageChops.difference(im1, im2)
bbox = diff.getbbox()
print("diff bbox:", bbox)
stat = ImageStat.Stat(diff)
print("diff mean:", [round(v,2) for v in stat.mean])

W, H = im2.size
cx, cy = W//2, H//2

def region_mean(img, box):
    return round(ImageStat.Stat(img.crop(box).convert("L")).mean[0], 1)

corners = {
    "tl": (10, 10, 200, 200),
    "tr": (W-200, 10, W-10, 200),
    "bl": (10, H-200, 200, H-10),
    "br": (W-200, H-200, W-10, H-10),
}
print("--- after corners luminance ---")
for k, box in corners.items():
    print(k, region_mean(im2, box))
print("--- before corners luminance ---")
for k, box in corners.items():
    print(k, region_mean(im1, box))

print("--- center luminance ---")
print("after center:", region_mean(im2, (cx-260, cy-200, cx+260, cy+200)))
print("before center:", region_mean(im1, (cx-260, cy-200, cx+260, cy+200)))

# 中心区域行扫描：找对话框卡片上下边界（亮度跳变）
col = cx
prev = None
rows = []
for y in range(0, H, 4):
    lum = region_mean(im2, (col-2, y, col+2, y+4))
    rows.append((y, lum))
# 找中心垂直线上亮度 > 100 的连续段（对话框卡片较亮）
seg = []
segments = []
for y, lum in rows:
    if lum > 110:
        seg.append(y)
    else:
        if len(seg) >= 3:
            segments.append((seg[0], seg[-1], len(seg)))
        seg = []
if len(seg) >= 3:
    segments.append((seg[0], seg[-1], len(seg)))
print("bright vertical segments at center col:", segments[:10])
