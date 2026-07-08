---
title: "คู่มือ: แปล Assassin's Creed Origins"
aliases:
  - AC Origins guide
  - Origins how-to
  - แปล Origins
tags:
  - type/guide
  - engine/anvilnext
  - game/assassins-creed
status: implemented
created: 2026-07-08
related:
  - "[[anvilnext-locpackage-format]]"
  - "[[anvilnext-forger]]"
---

# คู่มือ: แปล Assassin's Creed Origins ด้วยแอปนี้

คู่มือทำทีละขั้น จบครบ loop: แกะข้อความจากเกม → แปลในแอป → ยัดกลับเข้าเกม

---

## ภาพรวม — ทำไมต้องหลายขั้น

เกม Origins **ไม่ได้เก็บข้อความเป็น text ธรรมดา** มันซ่อนในไฟล์ที่ถูกห่อ 3 ชั้น
เหมือนของขวัญห่อ 3 ชั้น:

```
DataPC.forge                       ← กล่องใหญ่สุด (รวมทุกอย่างในเกม, หลาย GB)
  └─ NNN-…English_Subtitles.data   ← กล่องกลาง
       └─ 0-…Localization_Package  ← กล่องใน (ข้อความอยู่ในนี้ แต่เป็นรหัส binary อ่านไม่ออก)
            └─ "You must choose, Quick!"   ← ข้อความจริง
```

ต้องแกะทีละชั้นจนเจอ text → แปล → ห่อกลับทีละชั้นให้เหมือนเดิม

**แต่ละชั้นมีเครื่องมือของมัน:**

| ชั้น | ไฟล์ | แกะออก | ห่อกลับ |
|------|------|--------|---------|
| ใหญ่ | `.forge` | `Ubisoft_Forge_Tool.exe -e` | `Ubisoft_Forge_Tool.exe -i` |
| กลาง | `.data` | `Ubisoft_DATA_Tool.exe 11 -e` | `Ubisoft_DATA_Tool.exe 11 -i` |
| ใน | `.Localization_Package` | `aclocexport.exe` | `aclocimport.exe` |

> `11` = GameCode ของ Origins (Odyssey = `12`). `-e` = export (แกะ), `-i` = import (ห่อกลับ)

**งานแบ่ง 3 ช่วง:**

```
ช่วง 1 (แกะ)    : เกม → แกะ 3 ชั้น → LocalizationData.txt      ← คุณรัน tool เอง
ช่วง 2 (แปล)    : เปิด .txt ในแอปนี้ → แปล → Export             ← งานของแอป
ช่วง 3 (ห่อกลับ) : txt แปลแล้ว → ห่อ 3 ชั้น → ใส่กลับเกม          ← คุณรัน tool เอง
```

---

## สิ่งที่ต้องเตรียม

เครื่องมือ (โหลดจาก community เอง แล้ว **สแกนไวรัสก่อนรัน** — เป็น .exe ของคนอื่น):

- **Delutto Ubisoft Forge Tool** (`Ubisoft_Forge_Tool.exe`) — แกะ/ห่อ `.forge`
- **Delutto Ubisoft DATA Tool** (`Ubisoft_DATA_Tool.exe`) — แกะ/ห่อ `.data`
- **aclocexport.exe** + **aclocimport.exe** — แกะ/ห่อ `.Localization_Package`
  อยู่ในไฟล์ **`工具/aclocalizationpackagetool.7z`** ของรีโป community
  `MOIX-1192/assassin-s-creed-localization-texts` (แตก `.7z` จะได้ 2 exe นี้).
  รีโปเดียวกันยังแถม `Ubisoft_*_Tool_By_Delutto.7z` + `quickbms.zip` (ไว้ Valhalla)

คำสั่งข้างล่าง**ยืนยันจาก `Readme.txt` ของ tool เอง** (Delutto). กฎ 2 ข้อที่สำคัญ:

1. **ต้องใส่ `<OutputFolder>` เสมอ** — ทั้ง `-e` และ `-i` ใช้ argument ชุดเดียวกัน
   (tool ต้องอ่านโครงสร้างจากไฟล์ original เพื่อสร้างไฟล์ใหม่)
2. **`-i` (import) ไม่ทับไฟล์เดิม** — มันสร้างไฟล์ใหม่ลงท้าย **`.NEW`**
   (เช่น `DataPC.forge.NEW`) → คุณต้อง rename `.NEW` ทับไฟล์เดิมเอง

> รันไม่ใส่ argument = tool เปิด **dialog** ให้เลือกไฟล์แบบ GUI ได้เหมือนกัน

**GameCode ของ DATA Tool** (เลขหน้าคำสั่ง — เกม AC ที่รองรับ):

| Code | เกม | Code | เกม |
|------|-----|------|-----|
| 9 | Unity | 10 | Syndicate |
| **11** | **Origins** ← | 12 | Odyssey |
| 7 | IV Black Flag | 8 | Rogue |

(Valhalla ไม่อยู่ใน list → ต้องใช้ tool อื่น เช่น QuickBMS)

---

## ช่วง 0 — ลองแอปก่อน (ยังไม่ต้องมีเกม) ✅ แนะนำเริ่มตรงนี้

พิสูจน์ว่า "ช่วง 2" ใช้ได้ ก่อนไปยุ่งกับเกมจริง มีไฟล์ Origins จริงเตรียมไว้แล้ว:

1. เปิดแอป: `pnpm tauri dev`
2. Open project → เลือกโฟลเดอร์ที่มี `LocalizationData.txt`
   (ไฟล์ทดสอบจริง 33,787 บรรทัด — subtitle Origins ของจริง)
3. แอปควรขึ้น engine **"Assassin's Creed (aclocexport text)"** + เห็น units เต็ม
4. ลองแปลสัก 2-3 บรรทัด หรือกด Run (AI) → Export
5. เปิดไฟล์ที่ export ดู: ยังเป็นรูปแบบ `Id: [0x……]` เดิม แค่บรรทัดข้อความเปลี่ยน

ผ่าน = แอปพร้อม เหลือแค่ช่วง 1 + 3 กับเกมจริง

---

## CLI หรือ GUI ก็ได้

Delutto tool 2 ตัว (Forge/DATA) รันได้ 2 แบบ — **GUI ง่ายกว่า**:

**Ubisoft FORGE Tool (GUI):** double-click เปิด → ปุ่ม **Export** (แกะ) / **Import**
(ห่อ). กด → เลือกไฟล์ `.forge` → เลือก folder ปลายทาง.

**Ubisoft DATA Tool (GUI):** double-click → **เลือก dropdown "Select the game…" =
Assassin's Creed Origins ก่อน** (= GameCode 11) → แล้วค่อยกด Export/Import. ถ้าไม่
เลือกเกม tool ไม่รู้ format → พัง.

`aclocexport`/`aclocimport` มี **CLI อย่างเดียว** (ไม่มี GUI).

คำสั่ง CLI ด้านล่างใช้เมื่ออยากทำแบบ script/อัตโนมัติ.

## ช่วง 1 — แกะข้อความออกจากเกม

### 1.1 หาไฟล์เกม + backup ⚠️
Steam: `steamapps\common\Assassin's Creed Origins\`
ไฟล์หลัก = `DataPC.forge` (+ อาจมี `DataPC_patch_*.forge`)

**COPY `DataPC.forge` เก็บไว้ที่ปลอดภัยก่อน** (ไฟล์หลาย GB — พังแล้วต้องลงเกมใหม่)

### 1.2 แกะกล่องใหญ่ `.forge`
```
Ubisoft_Forge_Tool.exe -e DataPC.forge DataPC
```
(`DataPC` ท้ายคำสั่ง = ชื่อโฟลเดอร์ output)
ได้: โฟลเดอร์ `DataPC\` มีไฟล์ `.data` เยอะมาก มองหา 2 อันนี้:
- `NNN-LocalizationPackage_English_Subtitles.data` ← บทพูด/ซับ
- `NNN-LocalizationPackage_English.data` ← เมนู/UI

(`NNN` = เลขนำหน้า เช่น `393-…`)

### 1.3 แกะกล่องกลาง `.data`
```
Ubisoft_DATA_Tool.exe 11 -e NNN-LocalizationPackage_English_Subtitles.data NNN-LocalizationPackage_English_Subtitles
```
(`11` = Origins, ตัวท้าย = โฟลเดอร์ output)
ได้: โฟลเดอร์ที่มี `0-…English_Subtitles.Localization_Package` (ยังเป็น binary อ่านไม่ออก)

### 1.4 แกะกล่องใน → เป็น text
usage ยืนยันจากตัว exe เอง (`aclocexport.exe v0.2`): **1 argument = ไฟล์ package**
```
aclocexport.exe 0-…English_Subtitles.Localization_Package
```
ได้: **`0-…English_Subtitles.Localization_Package.txt`** ← ไฟล์นี้แอปอ่านได้ ✅
(output = ชื่อ input + `.txt`; ชื่อ "LocalizationData.txt" ในคู่มือคือชื่อตัวอย่างเฉยๆ)

> - input ต้องเป็นตัวที่ **DATA_Tool แกะแล้ว** (ขั้น 1.3) — ถ้าใส่ผิดไฟล์ exe จะฟ้อง
>   `Wrong header. Decompress LocalizationData using Delutto Ubisoft_DATA_Tool`
> - แอป detect จาก **เนื้อหา** (บรรทัดแรก `Id: [0x…]`) ไม่สนชื่อไฟล์ → rename ได้อิสระ
> - `aclocimport.exe` (ขั้น 3.1) เป็นคู่กัน: `aclocimport.exe <file>.txt` → `<file>.txt.out`

เปิดดูควรเห็นแบบนี้:
```
Id: [0x000D1792]
You must choose, Quick!

Id: [0x000D197F]
How did you get past the guard?
```

> อยากแปลทั้ง UI ด้วย ทำ 1.3–1.4 ซ้ำกับ `…English.data`

---

## ช่วง 2 — แปลในแอป

1. เปิดแอป → Open project → เลือกโฟลเดอร์ที่มี `LocalizationData.txt`
2. แอป detect เอง → engine **"Assassin's Creed (aclocexport text)"**
3. เห็นตารางข้อความทั้งหมด → แปล:
   - พิมพ์เองทีละบรรทัด **หรือ**
   - ตั้งค่า AI provider แล้วกด **Run** แปลทั้งชุด
4. เสร็จ → **Export**
5. ได้ `LocalizationData.txt` ที่แปลแล้ว (รูปแบบ `Id: [0x…]` เดิม เป๊ะ)

> markup พวก `<i>`, `<LF>`, `[beat]` แอปซ่อนให้ AI อัตโนมัติ ไม่ต้องแตะ — มันจะกลับมาครบ

---

## ช่วง 3 — ห่อกลับ + ใส่เข้าเกม

ทำย้อนช่วง 1 (ห่อกลับทีละชั้น):

> **จำกฎ:** `-i` สร้างไฟล์ `.NEW` เสมอ (ไม่ทับเอง) → ทุกขั้นต้อง rename `.NEW` ทับไฟล์เดิม
> และ `-i` ต้องชี้ไฟล์ **original** + ใส่โฟลเดอร์ที่เพิ่งแก้ (argument ชุดเดียวกับ `-e`)

### 3.1 ห่อกล่องใน (text → binary)
```
aclocimport.exe LocalizationData.txt
```
ได้: `LocalizationData.txt.out` (binary ที่แปลแล้ว)

### 3.2 เอาไปทับตัวเดิม
rename `LocalizationData.txt.out` → ทับไฟล์ `0-…Localization_Package` เดิม
(อยู่ในโฟลเดอร์ที่ได้จากขั้น 1.3)

### 3.3 ห่อกล่องกลาง
```
Ubisoft_DATA_Tool.exe 11 -i NNN-LocalizationPackage_English_Subtitles.data NNN-LocalizationPackage_English_Subtitles
```
ได้: `NNN-…English_Subtitles.data.NEW` → **rename ทับ `.data` เดิม** ในโฟลเดอร์ `DataPC\`

### 3.4 ห่อกล่องใหญ่
```
Ubisoft_Forge_Tool.exe -i DataPC.forge DataPC
```
ได้: `DataPC.forge.NEW` → **rename เป็น `DataPC.forge`** → วางใน game dir (ทับตัวเดิมที่ backup ไว้)

### 3.5 เล่น
เปิดเกม → ตั้งภาษา = **English** → เห็นภาษาไทย
(เพราะเราทับช่อง "English" ด้วยไทย — วิธีเดียวกับ mod ไทยที่มีคนทำ)

---

## ⚠️ เรื่อง Font — ไทยอาจขึ้นเป็น □□□

Origins จะ render ไทยได้ต่อเมื่อ font ในเกมมี glyph ไทย ถ้าไม่มี = ขึ้นเป็นสี่เหลี่ยม (tofu)

- Mod ไทยที่มีอยู่แถม `FontACO.rar` มาด้วย = ต้องแทน font ในเกมด้วย
- Font ฝังอยู่ใน `.forge` (คนละที่กับ text) → เป็นขั้นตอนแยก
- **แอปนี้ยังไม่จัดการ font ให้ Origins** (path นี้ทำ text อย่างเดียว)
- ถ้าไทยเป็น tofu: หา font ไทยที่ community ทำไว้มาแทน หรือแจ้งไว้เป็นงานเพิ่ม
  (ทำ `embed_font` ให้ engine `ac-loctext`)

---

## สรุปคำสั่งย่อ (cheat sheet)

```
# ── แกะ ──  (arg สุดท้าย = โฟลเดอร์ output)
Ubisoft_Forge_Tool.exe -e DataPC.forge DataPC
Ubisoft_DATA_Tool.exe  11 -e NNN-…English_Subtitles.data NNN-…English_Subtitles
aclocexport.exe            0-…English_Subtitles.Localization_Package
#   → LocalizationData.txt   → แปลในแอป → Export

# ── ห่อกลับ ──  (-i สร้าง .NEW เสมอ → rename ทับเดิมทุกขั้น)
aclocimport.exe            LocalizationData.txt      # → .txt.out
#   rename .txt.out ทับ 0-…Localization_Package
Ubisoft_DATA_Tool.exe  11 -i NNN-…English_Subtitles.data NNN-…English_Subtitles
#   → .data.NEW  → rename ทับ .data เดิม
Ubisoft_Forge_Tool.exe -i DataPC.forge DataPC
#   → DataPC.forge.NEW  → rename เป็น DataPC.forge → วางกลับ game dir
#   เล่น (ภาษา = English)
```

---

## ตัวอย่างจริง — Uplay install (command → OUTPUT ทุกขั้น)

game dir (Uplay): `C:\Program Files (x86)\Ubisoft\Ubisoft Game Launcher\games\Assassin's Creed Origins`
forge หลักที่มี localization = **`DataPC.forge`**. Package numbers (คอนเฟิร์มจาก corpus):
**`393`** = English_Subtitles (บทพูด), **`401`** = English (UI/เมนู).

⚠️ **อย่าทำใน Program Files** (ต้อง admin + เสี่ยง) — copy forge ไปทำที่ work dir แยก

```powershell
# ── SETUP: copy forge ไป work dir (ต้นฉบับปลอดภัย) ──
mkdir "E:\Games\ac-work"
copy "C:\Program Files (x86)\Ubisoft\Ubisoft Game Launcher\games\Assassin's Creed Origins\DataPC.forge" "E:\Games\ac-work\"
```

| # | Command | OUTPUT |
|---|---------|--------|
| 1 | `cd "E:\Games\Tools\Ubisoft_Forge_Tool_By_Delutto"`<br>`.\Ubisoft_Forge_Tool.exe -e "E:\Games\ac-work\DataPC.forge" "E:\Games\ac-work\DataPC"` | โฟลเดอร์ `E:\Games\ac-work\DataPC\` มี `.data` เพียบ (รวม `393-LocalizationPackage_English_Subtitles.data`) |
| 2 | `cd "E:\Games\Tools\Ubisoft_DATA_Tool_By_Delutto"`<br>`.\Ubisoft_DATA_Tool.exe 11 -e "E:\Games\ac-work\DataPC\393-LocalizationPackage_English_Subtitles.data" "E:\Games\ac-work\393_sub"` | `E:\Games\ac-work\393_sub\0-LocalizationPackage_English_Subtitles.Localization_Package` |
| 3 | `cd "E:\Games\Tools\aclocalizationpackagetool"`<br>`.\aclocexport.exe "E:\Games\ac-work\393_sub\0-LocalizationPackage_English_Subtitles.Localization_Package"` | `…\393_sub\0-…Localization_Package.txt` ← **ได้ .txt** |
| 4 | เปิดแอป → open folder `E:\Games\ac-work\393_sub` → แปล → Export | `.txt` ที่แปลแล้ว (ทับที่เดิม) |
| 5 | `cd "E:\Games\Tools\aclocalizationpackagetool"`<br>`.\aclocimport.exe "E:\Games\ac-work\393_sub\0-…Localization_Package.txt"` | `…\0-…Localization_Package.txt.out` → **rename ทับ** `0-…Localization_Package` เดิม |
| 6 | `.\Ubisoft_DATA_Tool.exe 11 -i "E:\Games\ac-work\DataPC\393-…English_Subtitles.data" "E:\Games\ac-work\393_sub"` | `…\DataPC\393-…Subtitles.data.NEW` → **rename ทับ** `.data` เดิม |
| 7 | `.\Ubisoft_Forge_Tool.exe -i "E:\Games\ac-work\DataPC.forge" "E:\Games\ac-work\DataPC"` | `E:\Games\ac-work\DataPC.forge.NEW` → **rename เป็น** `DataPC.forge` |
| 8 | copy `DataPC.forge` กลับ game folder (ทับ — ต้นฉบับ backup แล้ว) → เล่น ภาษา=English | เห็นไทย |

- **ทำ UI/เมนูด้วย:** ทำ #2–#7 ซ้ำกับ `401-LocalizationPackage_English.data`
- ถ้าเลข `393`/`401` ไม่ตรงหลัง #1 → หาไฟล์ชื่อ `*LocalizationPackage_English_Subtitles.data`

## แก้ปัญหา

| อาการ | สาเหตุ / วิธีแก้ |
|-------|----------------|
| แอป detect ไม่เจอ | ไฟล์ต้องขึ้นต้นบรรทัดแรกด้วย `Id: [0x……]` — ถ้าไม่ใช่ แสดงว่า aclocexport ยังไม่เสร็จ/ผิดไฟล์ |
| ไทยขึ้น □□□ ในเกม | font ไม่มี glyph ไทย → ดูหัวข้อ Font |
| เกม crash / ข้อความหาย | ห่อกลับผิดชั้น หรือ import ผิด GameCode (ต้อง `11` สำหรับ Origins) → เอา backup มาวางคืน แล้วทำใหม่ |
| `not recognized` ตอนรัน .exe | ต้องใส่ `.\` นำหน้าใน PowerShell เช่น `.\Ubisoft_Forge_Tool.exe …` |

---

## ดูเพิ่ม
- [[anvilnext-locpackage-format]] — รายละเอียด format + engine `ac-loctext`
- [[anvilnext-forger]] — เกม AC อื่น (Odyssey/Valhalla) ที่ใช้ `.acod`
- [[ENGINES]] — ตารางสรุป engine ทั้งหมด
