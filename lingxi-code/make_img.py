from PIL import Image, ImageDraw

img = Image.new("RGB", (300, 120), (255, 255, 255))
d = ImageDraw.Draw(img)
d.text((20, 40), "TEST IMAGE 123", fill=(220, 30, 30))
d.rectangle([10, 10, 290, 110], outline=(0, 0, 0))
img.save("/tmp/test_img.png")
print("created /tmp/test_img.png")
