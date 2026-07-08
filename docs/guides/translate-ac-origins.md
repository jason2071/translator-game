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

# คู่มือ: แปล Assassin's Creed Origins

แกะข้อความจากเกม → แปลในแอป (engine `ac-loctext`) → ห่อกลับ. External tool คุณรันเอง,
แอปแปล `.txt`.

## Tools (โหลด + สแกนไวรัสเอง — .exe ของ community)

- **Delutto Forge Tool** + **DATA Tool** — แกะ/ห่อ `.forge` / `.data`
- **aclocexport** + **aclocimport** — แกะ/ห่อ `.Localization_Package`
  (อยู่ใน `工具/aclocalizationpackagetool.7z` ของรีโป `MOIX-1192/assassin-s-creed-localization-texts`)

> `11` = GameCode ของ Origins. arg สุดท้าย = โฟลเดอร์ output (ต้องใส่เสมอ).
> `-i` (ห่อกลับ) สร้างไฟล์ `.NEW` → ต้อง rename ทับเอง (คำสั่งด้านล่างมี `copy … -Force` ให้แล้ว).

## ตัวอย่างจริง — Uplay install (copy-paste ทีละ step)

Package numbers (คอนเฟิร์มจาก corpus): **`393`** = English_Subtitles (บทพูด),
**`401`** = English (UI/เมนู). ⚠️ ทำใน work dir แยก (อย่าแตะ Program Files ตรงๆ).

**Step 0 — ตั้ง path ครั้งเดียว** (copy ทั้งก้อน วางใน PowerShell — step อื่นใช้ตัวแปรนี้):
```powershell
$game  = "C:\Program Files (x86)\Ubisoft\Ubisoft Game Launcher\games\Assassin's Creed Origins"
$work  = "E:\Games\ac-work"
$tools = "E:\Games\Tools"
$data  = "$work\DataPC\393-LocalizationPackage_English_Subtitles.data"
$pkg   = "$work\393_sub\393-LocalizationPackage_English_Subtitles\0-LocalizationPackage_English_Subtitles.Localization_Package"
mkdir $work -Force; copy "$game\DataPC.forge" $work
```

**Step 1 — forge → .data**
```powershell
& "$tools\Ubisoft_Forge_Tool_By_Delutto\Ubisoft_Forge_Tool.exe" -e "$work\DataPC.forge" "$work\DataPC"
```
→ `$work\DataPC\` เต็มไปด้วย `.data`

**Step 2 — .data → .Localization_Package**
```powershell
& "$tools\Ubisoft_DATA_Tool_By_Delutto\Ubisoft_DATA_Tool.exe" 11 -e $data "$work\393_sub"
```
→ ได้ไฟล์ที่ `$pkg` (DATA_Tool ซ้อน folder อีกชั้น — `$pkg` ชี้ครบแล้ว)

**Step 3 — .Localization_Package → .txt** ← ขั้นที่ทำ `.txt`
```powershell
& "$tools\aclocalizationpackagetool\aclocexport.exe" $pkg
```
→ ได้ `$pkg` + `.txt`

**Step 4 — แปลในแอป**
เปิดแอป → open folder `$work\393_sub\393-LocalizationPackage_English_Subtitles` → แปล → Export
(`.txt` แปลแล้วทับที่เดิม)

**Step 5 — .txt → binary (ห่อกลับ)**
```powershell
& "$tools\aclocalizationpackagetool\aclocimport.exe" "$pkg.txt"
copy "$pkg.txt.out" $pkg -Force
```
→ `.txt.out` เขียนทับ `.Localization_Package` เดิม

**Step 6 — binary → .data**
```powershell
& "$tools\Ubisoft_DATA_Tool_By_Delutto\Ubisoft_DATA_Tool.exe" 11 -i $data "$work\393_sub"
copy "$data.NEW" $data -Force
```
→ `.data.NEW` เขียนทับ `.data`

**Step 7 — .data → forge**
```powershell
& "$tools\Ubisoft_Forge_Tool_By_Delutto\Ubisoft_Forge_Tool.exe" -i "$work\DataPC.forge" "$work\DataPC"
```
→ ได้ `$work\DataPC.forge.NEW`

**Step 8 — ติดตั้ง + เล่น**
```powershell
copy "$work\DataPC.forge.NEW" "$game\DataPC.forge" -Force
```
→ เปิดเกม ตั้งภาษา = **English** → เห็นไทย

> - **ทำ UI/เมนูด้วย:** ทำ Step 2–7 ซ้ำ เปลี่ยน `393` → `401` (`English.data`)
> - เลขไม่ตรงหลัง Step 1 → หาไฟล์ชื่อ `*LocalizationPackage_English_Subtitles.data` ใน `$work\DataPC\`

## Gotchas

- **`File open error`** ตอน aclocexport → path ไม่ครบชั้น (DATA_Tool ซ้อน folder). ตัวแปร `$pkg` ชี้ถูกแล้ว
- **ไทยขึ้น □□□ (tofu)** → game font ไม่มี glyph ไทย, ต้องแทน font (community มี `FontACO.rar`). แอปยังไม่จัดการ font ให้ Origins
- **เกม crash / ข้อความหาย** → import ผิด GameCode (ต้อง `11`) หรือห่อผิดชั้น → เอา backup วางคืน แล้วทำใหม่
- **`not recognized`** ตอนรัน .exe → ใส่ `.\` นำหน้าใน PowerShell

## ดูเพิ่ม
- [[anvilnext-locpackage-format]] — format + engine `ac-loctext`
- [[anvilnext-forger]] — AC Odyssey/Valhalla (`.acod`)
