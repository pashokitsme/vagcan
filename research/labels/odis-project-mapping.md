# VW's project names, and which vehicles each covers

**What this is.** `S42 — Fahrzeugprojektzuordnung` (vehicle-project mapping), version
3.0.0, transcribed from VW's own HTML rendering into markdown. It is the authority for
what an ODIS project *name* means: `SK37X` is not a car, it is a **platform**, and this
table is what says which vehicles fall under it.

**165 projects, 528 vehicles, ten brands.**

**What this is not.** It is **not a runtime input, and nothing reads it.** A project
declares its own coverage — `PRNR-INFO.xml` inside every extracted ODIS project lists
each vehicle it covers with a `PRODUCT-ID` (the VW type code, `5E0`, `55A`, `5EU`), and
that file is what `vag_data::odis::Project::vehicles` parses. This table is the human's
copy, for reading a project name and knowing what it is; the machine never consults it.
The S42 lines below are, entry for entry, a rendering of the same `PRNR-INFO.xml` data —
verified on `SK37X`, whose six vehicles match exactly.

**How to read an entry.** `SK37x/0EU K_5E0 A7 / Octavia III (Limo, Combi)` is
`<vehicle-project>` + `<product-id>` + `<name>` run together: project `SK37X/0EU_X`,
type code `5E0`, an Octavia III saloon or estate. The type code is the key — it is what a
car has to be matched against for a project to be chosen for it.

**Two cautions, both load-bearing:**

- **A type code is a property of the car, not of a part.** A part number's leading three
  characters name the platform *the part* belongs to, which is often not the car's:
  the reference Octavia III (`5E0`) carries an engine controller numbered `8V0` — an
  Audi A3 number — and `5Q0`/`3Q0` parts shared across the whole MQB group. Three of its
  fifteen control units report `5E0`; the rest do not.
- **Nothing here proves the type codes are disjoint across projects.** They are within
  this table as printed, but a second extracted project is what would confirm that no
  two claim the same code. Until then, treating a code as selecting exactly one project
  is an assumption, not a fact.

`[EOP]` marks a project VW has ended production for; `VERALTET` is "obsolete".

---

### Audi

| project | vehicles it covers |
|---|---|
| `AU21X` | AU210/0_8XE_8XE A1 E-TRON<br>AU210/0_FZ0 A1PA 3T [EOP]<br>AU210/1_FZS A1PA 5T [EOP]<br>AU210/x_8X0_8X0 A1 [EOP]<br>AU316/0_8U0 Q3 [EOP]<br>AU316/0CN_8UD Q3 China [EOP] |
| `AU27X` | AU270/1 A1NF 5T |
| `AU35X` | AU350/0_8P0 AB2 [EOP]<br>AU355/0_8PC AB2 Cabrio [EOP]<br>AU42x/1_8J0 TT2 Coupe/Roadster NF<br>AU61x/x_420 R8 [EOP] |
| `AU37X` | AU276/0_B_GAG BL-X55 BEV<br>AU276/0_GA0 Q2<br>AU276/0CN_GAD Q2 China<br>AU326/0_F30 Q3<br>AU326/0CN_F3G Q3 China<br>AU326/1_F3A Q3 Sportback<br>AU371/0LA_8VB AB3 LA [EOP]<br>AU375/0_8VC AB3 Cabrio [EOP]<br>AU37x/x_8V0 AB3 [EOP]<br>AU37x/xCN_8VD AB3 China [EOP]<br>AU434/1_FV0 TT3 Coupe<br>AU435/1_FVR TT3 Roadster |
| `AU38X` | AU380/1_8YF AB4 Sportback<br>AU380/1CN_K_8YG AB4 Sportback China<br>AU381/0_8Y0 AB4 Limousine<br>AU381/0CN_K_8YD AB4 Sportlimousine China<br>AU516/3CS_4CG C-SUV |
| `AU38X-PA` | AU336/0CN_F3H Q3NF China<br>AU336/0EU_K_F31 Q3NF<br>AU336/1CN_F3K Q3NF Sportback LWB<br>AU336/1EU_K_F3B Q3NF Sportback<br>AU380/1__8YFVB VERALTET*<br>AU380/1_8YF AB4 Sportback<br>AU380/1CN_K_8YG AB4 Sportback China<br>AU381/0_8Y0 AB4 Limousine<br>AU381/0CN_K_8YD AB4 Sportlimousine China<br>AU416/6CS_K_F4E B-SUV Q4 CS MQBBL<br>AU516/3CS_4CG C-SUV<br>AU516/3CSBL_4CGVB VERALTET |
| `AU40X` | AU401/0_EU_8WH B10 (A5) Limousine<br>AU402/0_EU_8WK B10 (A5) Avant / Allroad<br>AU436/0ME_FYA Q5NF_NF<br>AU591/0_4P0 (C9) A6 Limousine<br>AU591/0CN_4PD (C9) A6 Limo China LWB<br>AU592/x_4PA (C9) A6 Avant/Allroad |
| `AU48X` | AU416/0_8R0_8R0 Q5 [EOP]<br>AU416/0_8RD Q5 China [EOP]<br>AU481/0_8KD_8KD B8 China [EOP]<br>AU485/0_8F0_8F0 B8 Cabrio [EOP]<br>AU48x/x_8K0_8K0 B8 Lim/Av/Allroad [EOP]<br>AU48x/x_8T0_8T0 B8 Coupe/ Sportback [EOP] |
| `AU49X` | AU426/0CN_FYD Q5NF China<br>AU426/0ME_FY0 Q5NF<br>AU426/1CN_FYG Q5 Sportback China<br>AU426/1ME_FYF Q5 Sportback<br>AU491/0_8W0 B9 Limousine<br>AU491/0CN_8WD B9 China<br>AU492/0_8WA B9 Avant/ Allroad<br>AU493/0_8WF B9 Sportback<br>AU494/0_8WB B9 Coupe<br>AU495/0_8WC B9 Cabrio |
| `AU56X` | AU516/0_4L0_4L0 Q7 [EOP]<br>AU561/1_4FD_4FD C6 China [EOP]<br>AU56x/x_4F0_4F0 C6 Lim/Av/Allroad [EOP] |
| `AU57X` | AU571/0_4GD C7 China [EOP]<br>AU573/0_4G8 A7 Sportback [EOP]<br>AU57x/x_4G0 C7 Lim/Av/Allroad [EOP] |
| `AU58X` | AU512/2 e-tron GT Avant<br>AU513/1_4KH Q8 e-tron sport sedan (ESS)<br>AU513/2_4J1 e-tron GT<br>AU516/1_4KE Q8 e-tron<br>AU516/1CN_4KC e-tron CN CKD<br>AU536/3_4MF Q8<br>AU581/0_4K0 C8 Limousine<br>AU581/0CN_4KD C8 Limousine China (A6)<br>AU581/1CS_4KG C8 Limousine China (A7)<br>AU582/0_4KA C8 Avant / Allroad<br>AU583/0_4KF C8 Sportback<br>AU584/0_4KB_4KB C8 Coupe [EOVM] |
| `AU64X` | AU641/0_4H0 D4 [EOP] |
| `AU65X` | AU651_MLB65 Technikträger<br>AU651/0EU_C_4N6 D5 SSF<br>AU651/0EU_K_4N0 D5 NWB<br>AU651/0EU_L_4N4 D5 LWB |
| `AU724` | AU624/0_4S0 R8NF Coupe<br>AU624/2_4SB R8NF GT4<br>AU624/3_4SC R8 LMS GT2<br>AU625/0_4SR R8NF Spyder |
| `AU73X` | AU536/0_4M0 Q7NF |
| `AU924` | AU614/1_4J0 RSE-ETRONAB2 [EOVM]<br>AU624/1_4JE R8 etron 2.0 [EOVM] |
| `AUE31` | AU310/6_EU_10N E3 CUV MEB<br>AU316/2_/4_89A A-SUVe / A-CUVe<br>AU316/2CN_89G A-SUVe China Nord<br>AU316/3CS_89D A+SUVe China Süd |
| `AUE41` | AU416/2_F5S eQ5 SUV (Q6 e-tron)<br>AU416/2CE_B_F5D Q6 e-tron LWB 85D China<br>AU416/3CE_B_F5G Q6 Sportback e-tron LWB 85G China<br>AU416/3EU_F5R eQ6 (Q6 Sportback e-tron)<br>AU511/4_CE_F5H E6 Limo LWB China<br>AU512/4_F5A E6 Avant<br>AU513/4_F5F E6 Sportback |
| `AUEXT` | AUext Fahrzeug externe Komponenten |

### Bugatti

| project | vehicles it covers |
|---|---|
| `BG734` | BG724_5B0 Veyron<br>BG725_5B1 Veyron Grand Sport<br>BG734_5B2 Veyron Super sport<br>BG735_5B3 Vitesse |
| `BG744` | BG744_5B4 Chiron<br>BG755_5B5 Bolide_Mistral |

### Bentley

| project | vehicles it covers |
|---|---|
| `BY62X` | BY61x_3W0 Bentley 61x<br>BY621_4W0 4 dr Continental<br>BY624/5_390 Continental |
| `BY636` | BY636_4V0 Bentayga<br>BY636_EWB_4V1 Bentayga EWB |
| `BY63X` | BY631_371 BY631 Flying Spur<br>BY631 FL_373 Flying Spur FL<br>BY634/5_370 Continental (63x)<br>BY634/5 FL_372 Continental FL |
| `BY64X` | BY646_1_4V7 D-LUV<br>BY646_2_4V8 D-SUV |
| `BY73X` | BY731_3Y0 Mulsanne |
| `BYEXT` | BYEXT BYext |
| `BYTEST` | BYTest BYTest |

### Lamborghini

| project | vehicles it covers |
|---|---|
| `LB634` | LB634_4LA Huracán NF<br>LB635_4LB Huracán NF Spyder |
| `LB636` | LB636_4ML Urus |
| `LB72X` | LB624_4T0 Huracán Coupe<br>LB624/1EU_K_4T1 Huracan Coupé STO<br>LB625_4TR Huracán Spyder |
| `LB744` | LB744_47B Revuelto |
| `LB83X` | LB73x_100_47A Centenario<br>LB73x_470 Aventador<br>LB73x_48V_47F SIAN |

### Porsche

| project | vehicles it covers |
|---|---|
| `PO416` | PO416 Macan |
| `PO51X` | PO513 J1 |
| `PO526` | PO526 Cayenne E2 |
| `PO53X` | PO536 Cayenne E3 |
| `PO62X` | PO623 Panamera G2 |

### Seat

| project | vehicles it covers |
|---|---|
| `SE120` | SE120/0EU B_12S eMii<br>SK120/0EU_B_12K e-Citigo<br>VW120/0_120 (SE120, SK120) up!, Mii, Citigo<br>VW120/0 _12E Elektro up!<br>VW120/X_12B NSF LA |
| `SE25X` | SE25X/X_6J0_6J0 Ibiza / Ibiza SC / Ibiza Sporttourer [EOP] |
| `SE25X1` | SK351/3EU K_605 Rapid, Rapid SB (SK350/3EU K), Toledo (SE351/3EU K)<br>SK351/3RU K_609 Rapid / A-Entry (RU) |
| `SE26X` | SE25X/X_6P0 (PQ26) Ibiza / Ibiza SC / Ibiza Sporttourer [EOP] |
| `SE27X` | SE27X/0_6F0 Ibiza / Arona |
| `SE35X` | SE350/0_1P0 Leon [EOP]<br>SE35X/X_5P0 Toledo / Altea / Altea XL / Altea Freetrack [EOP] |
| `SE36X` | SE428/0_710 Alhambra NF<br>VW316/0_EU_5N0 Tiguan [EOP]<br>VW350/0_1K0_1K0 A5 Golf [EOVM]<br>VW358/0_1T0 VW368 A5 Touran [EOVM]<br>VW360/0_5K0_5K0 A6 Golf [EOVM]<br>VW360/0_5KE_5KE A6 Golf Elektro [EOVM]<br>VW360/2_5M0_5M0 A5 Golf Plus [EOVM]<br>VW364/0_EU_130 SCIROCCO [EOVM]<br>VW365/0_5KK_5KK A6 Golf Cabrio[EOVM]<br>VW365/2_1Q0_1Q0 EOS [EOP]<br>VW428/0_EU_7N0 Sharan |
| `SE37X` | SE326/0EU_K_5FP Ateca<br>SE326/1_5FL Tarraco [EOP]<br>SE37X/X_5F0 Leon / Leon SC / Leon Sporttourer / Leon Xperience |
| `SE38X-PA` | SE316/3__5FFVB Formentor (PTT+TK+AGT)<br>SE316/3_5FF Formentor<br>SE336/0EU_57S Terramar<br>SE38X_5FNVB Leon (PTT+TK+AGT)<br>SE38X/0_5FN Leon |
| `SE38X` | SE316/3_5FF Formentor<br>SE38X/0_5FN Leon |
| `SE41X` | SE41X_3R0 Exeo [EOP] |
| `SEE21` | SE210/6_6FA Small BEV |
| `SEE31` | SE310/6_10S CUPRA Born |
| `SEE31-CM` | SE316/8CM_20H Tavascan<br>VW311/0_CM_10G A Entry Notchback VW Anhui<br>VW313/2_CM_11M A COSe VW Anhui<br>VW316/8_CM_11H A SUVe Black Label VW Anhui |

### Skoda

| project | vehicles it covers |
|---|---|
| `SK120` | SE120/0EU B_12S eMii<br>SK120/0EU_B_12K e-Citigo<br>VW120/0_120 (SE120, SK120) up!, Mii, Citigo<br>VW120/0 _12E Elektro up!<br>VW120/X_12B NSF LA |
| `SK25X-CS` | SK250CS_5JD A05 / Fabia ModF (SVW China) |
| `SK25X1` | SK351/3EU K_605 Rapid, Rapid SB (SK350/3EU K), Toledo (SE351/3EU K)<br>SK351/3RU K_609 Rapid / A-Entry (RU) |
| `SK2531-CS` | SK350/3CS K_608 Rapid SB / A-Entry (SVW China)<br>VW35X/1_604 (SK351/1CS) A-Entry China (SAIC-VW) |
| `SK25X` | SK250/2/7/8_5J0 A05 / Fabia II + Roomster<br>SK250 IN _5JF A05 / Fabia II FL (Indien)<br>SK250 RU_5JU A05 / Fabia II FL (Russland)<br>SK251/0IN K_607 Rapid (Indien) |
| `SK26X-CS` | SK260/0CS K_6VD A06 / Fabia III (SVW China) |
| `SK26X` | SK26x/0EU K_6VA A06 / Fabia III / Combi |
| `SK27X` | SK216/2IN_K_628 A0 SUV Kushaq<br>SK216/6IN_K_62E IN 2.5 (SUV)<br>SK270/0EU_6VNVB Fabia NF UNECE PROTOTYP<br>SK270/0EU_K_6VN Fabia NF<br>SK271/1IN_K_629 A0 NB Indien Slavia<br>SK370/xEU_K_655 Scala / Kamiq<br>SK370/xEU K_655VB Scala / Kamiq UNECE PROTOTYP<br>VW216_2GCVB T-Cross EU (Prototypen)<br>VW216/0_2GC T-Cross<br>VW216/2IN_K_623 _A0 SUV Taigun<br>VW246/5_SA_6S1 Entry A0 SUV_<br>VW261/0RU_K_620 Polo Sedan NF<br>VW270_2G0VB Polo 7 (Prototypen)<br>VW270/0__2G0 Polo 7<br>VW270/3_EU_2F0 Polo CUV Taigo<br>VW271/1IN_62A A0 NB Virtus<br>VW275/0 EU_2GK T-Roc Cabrio<br>VW276_UNECE_2GYVB T-Roc (Prototypen)<br>VW276/0_2GA T-Roc |
| `SK316-CS` | SK316/3_CS_18A CUV (SVW China) / SK316/4 Kamiq GT |
| `SK35X` | SK316/0EU K_5L0 A-SUV / Yeti (EU)<br>SK316/0RU K_5LU A-SUV / Yeti (Russland)<br>SK351/2_1Z0 A5 / Octavia II (EU)<br>SK351 RU_1ZU A5 / Octavia II (Russland) |
| `SK37X-CS` | SK326/0CS_B Karoq BEV (SVW China) / A-SUV<br>SK326/0CS_K_5EG Karoq (SVW China) / A-SUV<br>SK326/xCS _55C Kodiaq / Kodiaq GT (SVW China) / A-PlusSUV<br>SK371/0CS_B_5EE A7 / Octavia III BEV (SVW China)<br>SK371/0CS K_5ED A7 / Octavia III (SVW China) |
| `SK37X` | SK326/0EU_K_5EP Karoq (EU) / A-SUV<br>SK326/0EU K_5EPVB Karoq UNECE PROTOTYP<br>SK326/1EU_K_55A Kodiaq (EU) / A-PlusSUV<br>SK326/1RU_K_55U Kodiaq (RU) / A-Plus SUV<br>SK371/0IN K_5EF A7 / Octavia III. (Indien)<br>SK371/0RU K_5EU A7 / Octavia III. (Russland)<br>SK37x/0EU K_5E0 A7 / Octavia III (Limo, Combi) |
| `SK38X-CS` | SK381/0CS_K_5DD A8 / Octavia IV (SVW China) |
| `SK38X-PA` | SK336/1EU_57HVB Kodiaq NF UNECE PROTOTYP<br>SK336/1EU_K_57H Kodiaq NF<br>SK38x/0EU_5EN A8 / Octavia IV (Combi, Limo, PHEV, mHEV, RS, Allrad)<br>SK38X/0EU K_5ENVB A8 Octavia IV UNECE PROTOTYP |
| `SK38X` | SK38x/0EU_5EN A8 / Octavia IV (Combi, Limo, PHEV, mHEV, RS, Allrad) |
| `SK46X` | SK461/2_3T0 B6 / Superb II (EU) |
| `SK48X-CS` | SK481/0CS K_3VD B8 / Superb III (SVW China) |
| `SK48X` | SK48x/0EU_K_3V0 B8 / Superb III (Combi, Limo) |
| `SK49X` | SK49x/0EU_K_3P0 B9 / Superb IV (Combi, Limo)<br>SK49xEU_VB_3P0VB B9 Superb IV UNECE PROTOTYP<br>MQB(W)_49x_3J0VB VW49x__Passat_NF_AGT+PT<br>MQB48W_48WVB Baukasten [EOVM]<br>VW492/0_EU_3J0 B9 Passat |
| `SKE21` | SK216/1EU_B_3FA Small-BEV |
| `SKE31` | SK316/7EU__50BVB 7-Sitzer_VB<br>SK316/7EU_50B 7-Sitzer<br>SK316/xEU_B_50A MEB (Elroq, Enyaq - SLR/SLC)<br>SK316/xEU B_50AVB Enyaq UNECE AGT |
| `SKSVW` | SK316/0CSKL_5LD A-SUV / Yeti (SVW China)<br>SK351CS_K_1ZD A5 / Octavia II (SVW China)<br>SK461CS_3TD B6 / Superb ModS (SVW China) |

### Volkswagen NFZ

| project | vehicles it covers |
|---|---|
| `VN337E` | VN337/7 E-Caddy[EOVM] |
| `VN35S-PA` | VN35S/X__2KA Caddy 5<br>VN35S-PA_2KXVB Baseline |
| `VN35S` | VN35S/0EU_2KAVB Caddy 5 I-Stufen<br>VN35S/X__2KA Caddy 5 |
| `VN35X` | VN33S/0_2K0_2K0  Caddy [EOP]<br>VN33S/0_2KD_2KD Caddy China (FAW) [EOP]<br>VN34S/X_2KC_2KC Caddy 4 |
| `VN46X` | VN46T_7EPVB T6 PA geschlossene Aufbauten I-Stufen<br>VN46T _7FPVB T6 PA offene Aufbauten I-Stufen<br>VN46T/0_7FP T6PA offener Aufbau<br>VN46T/1_7EP T6PA geschlossener Aufbau<br>VN46T/2_7EM T6PA Multivan |
| `VN47X-PA` | VN41T__7T0 T7<br>VN41X-PA_7TXVB T7 Baseline |
| `VN47X` | VN41T__7T0 T7<br>VN41T_7T0VB "T7" geschlossene Aufbauten I-Stufen |
| `VN54M` | VN54M/X_7CM Pluto |
| `VN54X-PA` | VN54T/X__7CP Crafter NF offen<br>VN54T/X_7C0 Crafter NF geschlossen<br>VN54T-PA_7CXVB Crafter Baseline |
| `VN54X` | VN54T_7C0VB Crafter NF Prototypen / AGT I-Stufen[EOVM]<br>VN54T/X__7CP Crafter NF offen<br>VN54T/X_7C0 Crafter NF geschlossen<br>VN54T/X_7CE_7CEVB e-Crafter 1.5 I-Stufen<br>VN54T/X _7CE e-Crafter |
| `VN75X` | VN4XT/X__7F0 T6 Poznan [EOVM]<br>VN4XT/X_7E0 T6 [EOVM] |
| `VN81X` | VN417/X__2H0 RPU Amarok [EOVM]<br>VN417/X_2HA RPU Amarok GP<br>VN417/X__2H0 RPU Amarok [EOVM]<br>VN417/X_2HA RPU Amarok GP |
| `VN83X_VW` | VN53T/X_2E0 LT3 Crafter [EOVM] |
| `VNE41-AD` | VN41S___15BVB ID BuzzAD 2.0<br>VN41S__15B ID Buzz AD2.0 |
| `VNE41` | VN41S_15A ID Buzz |
| `VNE47` | VN41T/0_7TE T7 BEV [EOVM]<br>VN41T/2_7TEVB T7 BEV I-Stufen [EOVM] |

### Volkswagen

| project | vehicles it covers |
|---|---|
| `MEB` | MEB31_10AVB Modularer Elektrifizierungs-Baukasten<br>MEB41B_15AVB Modularer Elektrifizierungs-Baukasten |
| `MQBAB` | MQB27_2Q0VB Modularer Querbaukasten_A0[EOVM)<br>MQB27Global_2QBVB Modularer Querbaukasten__A0 [EOVM]<br>MQB37_2GAVB _SUV / VW276 Prototypen<br>MQB A1_5Q0VB Modularer Querbaukasten_A1 [EOVM]<br>MQB A2 Fkt. nur für Funktion [EOVM]<br>MQBA37_mHEV_550VB MQB37ASP_PHEV<br>MQB B_3Q0VB Modularer Querbaukasten_B [EOVM]<br>MQB B-SUV_3QFVB Modularer Querbaubasten_B-SUV |
| `VW019` | VW114/0_6Z0 XL1 [EOVM] |
| `VW120` | SE120/0EU B_12S eMii<br>SK120/0EU_B_12K e-Citigo<br>VW120/0_120 (SE120, SK120) up!, Mii, Citigo<br>VW120/0 _12E Elektro up!<br>VW120/X_12B NSF LA |
| `VW21X` | VW210/3_5Z0 Fox<br>VW218/0_5ZR Suran/Spacefox [EOVM] |
| `VW23X` | VW23X/x_5U0 Gol / Voyage / Saveiro |
| `VW250-CS` | VW250/0_60D Polo A05 China (SAIC-VW) |
| `VW2531-CS` | SK350/3CS K_608 Rapid SB / A-Entry (SVW China)<br>VW35X/1_604 (SK351/1CS) A-Entry China (SAIC-VW) |
| `VW2532-CN` | VW351/2_CN_603 Jetta A2 NF China (FAW VW) [EOP] |
| `VW25X` | VW250/0_6R0_6R0 EU/SA Polo A05 [EOVM]<br>VW250/2_6RS_6RS Polo Vivo SA<br>VW251/0_601 Polo Russland [EOVM]<br>VW25X/0___621 Polo Malaysia [EOVM]<br>VW25X/0__622 Polo Indien Compact Sedan (ICS) Ameo [EOVM]<br>VW25X/0_602 Polo Indien |
| `VW26X` | VW250/0_6C0_6C0 Polo GP [EOVM] |
| `VW27X-CN` | VW216/0_CN_671 T-Cross LWB (FAW VW) |
| `VW27X-CS` | VW216/0_CS_670 T-Cross LWB (SAIC-VW)<br>VW270/0_67D Polo 7 (SAIC-VW) |
| `VW27X-LA` | VW216_2GEVB_2GEVB T-Cross LA (Prototypen) [EOVM]<br>VW216/0_LA_2GE T-Cross_LA<br>VW246/4LA K_67EVB Polo SUV LA (Prototypen)<br>VW246/5_LA_2FF Entry A0 SUV<br>VW247/X_LA_2FC UDARA<br>VW270_67BVB_67BVB Polo CUV (Prototypen)<br>VW270/1LAKA_2FB Polo Track<br>VW270/1VB_2FBVB Polo Track (Prototypen)<br>VW270/3_LA_67B Polo CUV LA<br>VW27x_2GBVB_2GBVB Polo-G (Prototypen) [EOVM]<br>VW27X/2_LA_2GB Polo-G / Virtus |
| `VW27X` | VW216_2GCVB T-Cross EU (Prototypen)<br>VW216/0_2GC T-Cross<br>VW216/2IN_K_623 _A0 SUV Taigun<br>VW246/5_SA_6S1 Entry A0 SUV_<br>VW261/0RU_K_620 Polo Sedan NF<br>VW270_2G0VB Polo 7 (Prototypen)<br>VW270/0__2G0 Polo 7<br>VW270/3_EU_2F0 Polo CUV Taigo<br>VW271/1IN_62A A0 NB Virtus<br>VW275/0 EU_2GK T-Roc Cabrio<br>VW276_UNECE_2GYVB T-Roc (Prototypen)<br>VW276/0_2GA T-Roc |
| `VW311-1` | VW310/4_180 VW311/4_VW321/4 Lavida China (SAIC-VW) |
| `VW3112-CN` | VW311/5_150 VW320/5 _VW321/5 New Bora China (FAW VW) |
| `VW316-C` | VW316/0_CS_5ND Tiguan Lang China (SAIC-VW) [EOVM] |
| `VW32X` | VW32X/1_5C0_5C0 Beetle NF<br>VW35X/0_1KM_1KM A5 VW362 Jetta_Golf Variant [EOVM]<br>VW361/0_ME_160 Jetta NF [EOVM]<br>VW361/0_MY_16M Jetta Malaysia CKD [EOP]<br>VW361/0_RU_16R Jetta NF Russland [EOP] |
| `VW358-C` | VW358/0_1TD_1TD A5 Touran China (SVW) [EOVM] |
| `VW36X-CN` | VW351/0_1KD A5 Sagitar China (FAW) [EOVM]<br>VW360/0_5KD_5KD A6 Golf China (FAW) [EOVM]<br>VW361/0_CN_16D Sagitar NF (FAW VW) [EOP] |
| `VW36X` | SE428/0_710 Alhambra NF<br>VW316/0_EU_5N0 Tiguan [EOP]<br>VW350/0_1K0_1K0 A5 Golf [EOVM]<br>VW358/0_1T0 VW368 A5 Touran [EOVM]<br>VW360/0_5K0_5K0 A6 Golf [EOVM]<br>VW360/0_5KE_5KE A6 Golf Elektro [EOVM]<br>VW360/2_5M0_5M0 A5 Golf Plus [EOVM]<br>VW364/0_EU_130 SCIROCCO [EOVM]<br>VW365/0_5KK_5KK A6 Golf Cabrio[EOVM]<br>VW365/2_1Q0_1Q0 EOS [EOP]<br>VW428/0_EU_7N0 Sharan |
| `VW37X-CN` | VW276/0_CN_2GD T-Roc LWB (FAW VW)<br>VW326/3_CN_55G Tayron  (FAW VW)<br>VW331/5_15E New Bora BEV (FAW VW)<br>VW331/5_CN_15G A7 New Bora China (FAW VW)<br>VW370_CN_15DVB e-Golf (FAW VW) [EOVM]<br>VW370/0_15D_15D e-Golf China (FAW VW) [EOVM]<br>VW370/0_CN_5GG A7 Golf China (FAW VW)<br>VW370/2_CN_5GH A7 Golf Sportsvan China (FAW VW) EOP<br>VW371/0_CN_17G Sagitar China (FAW VW) |
| `VW37X-CS` | VW313/0_5GD Lamando China (SAIC-VW)<br>VW316/5_CS_2GG Tharu China (SAIC-VW)<br>VW316/5CS_2BEVB TharuBEV (SAIC) [EOVM]<br>VW316/5CS_B_2BE Tharu BEV (SAIC)<br>VW326/1_CS_55D A7 Tiguan China (SAIC-VW)<br>VW331/4_18D Lavida NF (SAIC-VW)<br>VW331/4_CS_18E New Lavida BEV (SAIC-VW)<br>VW371/8CS_K_67C Entry NB CS<br>VW378/0_CS_5TD A7 Touran China (SAIC-VW) |
| `VW37X` | VW316/5_ME_2GM Tarek NAR LA<br>VW316/5RU_K_2BR Taos RUS<br>VW326/0_EU_550 A7 Tiguan / MQB-A2 (SUV)<br>VW326/0_ME__55NVB A7 Tiguan ME PA Prototypen<br>VW326/0_ME_55N A7 Tiguan ME<br>VW326/0_RU_55R A7 Tiguan Russland<br>VW370/0_BEV_5GE A7 e-Golf Elektro<br>VW370/0_LA_5GB A7 Golf Brasilien [EOVM]<br>VW370/2_EU_5GP A7 Golf Sportsvan<br>VW371_17AVB_17AVB A7 Jetta Mexiko (Prototypen)<br>VW371/0_ME_17A A7 Jetta Mexiko<br>VW378_5T0VB_5T0VB Touran Prototypen<br>VW378/0_EU_5T0 A7 Touran / MQB-A2 (MPV)<br>VW37X/0_EU_5G0 A7 Golf<br>VW37X/0_ME_5GM A7 Golf Mexico [EOVM] |
| `VW38X-CN` | VW380/0_5HG A8 Golf China (FAW VW)<br>VW38x_5HGVB Golf  A8 China (FAW VW und SAIC) (EOVM) |
| `VW38X-CS` | VW323/0_CS_5DB Lamando NF<br>VW323/0 CS_5DBVB Lamando_NF [EOVM] |
| `VW38X-PA-CN` | VW336/3CN_K_57C Tayron NF (FAW VW)<br>VW341/5CN_K_16G Bora NF<br>VW380/0_5HG A8 Golf China (FAW VW)<br>VW380/0_CN_5H1VB Golf A8 PA China (FAW VW)<br>VW381/0CN_K_18G Sagitar A8<br>VW386/0_CN_3GF T-Roc LWB NF |
| `VW38X-PA-CS` | VW323/0_CS_5DB Lamando NF<br>VW3230CS_5DOVB Lamando NF Baseline<br>VW326/5_CS_2HH KL_Tharu NF<br>VW336/0_CS_57D Tiguan NF China<br>VW341/4CS_K_19D Lavida NF |
| `VW38X-PA-LA` | VW213/0LA_K_5ZA Nivus NF (A0 COS) |
| `VW38X-PA` | MQB(W)_336_570VB VW336__Tiguan_NF_AGT+PT<br>MQB(W)336_3_57LVB VW336_3_Tayron_EU_AGT+PT<br>MQB(W)336-3_57NVB VW336_3_Tayron_NAR_AGT+PT<br>MQB(W)UNECE_37WVB VW38x__Golf_PA_AGT+PT<br>VW336/0EU_K_570 Tiguan NF<br>VW336/3EU_K_57L Tayron EU<br>VW336/3ME_K_57N Tayron NAR<br>VW386/0EU_3GA T-Roc NF<br>VW38X/0_EU_5H0 A8 Golf |
| `VW38X` | MQB37(W)_5H0VB MQB2020 konv.+PHEV<br>MQB37A(W)_P5RVB Modularer Querbaukasten<br>VW38X/0_EU_5H0 A8 Golf |
| `VW411-CS` | VW411/1_56D New Midsize Sedan China (SAIC-VW) [EOVM] |
| `VW411` | VW411/1_NA_560 New Midsize Sedan (NAR) [EOP] |
| `VW416-CN` | VW416/2CN_K_30G B-SMV Talagon (FAW VW)<br>VW416/2CN-K_30GVB B_SMV (FAW VW)  [EOVM]<br>VW416/3CN_K_30C B-Main SUV (FAW VW)<br>VW416/3CN K_30CVB B Main SUV (FAW VW) [EOVM] |
| `VW416-CS` | VW416/0_3CG Teramont (SAIC-VW)<br>VW416/1_3CC B-SUV Coupe (SAIC-VW)<br>VW418/2_30D_30D K B-MPV (SAIC-VW)<br>VW418/2_30DVB K B-MPV (SAIC VW) [EOVM]<br>VW421/1_3GB New Midsize Sedan China NF (SAIC-VW) |
| `VW416-PA-CN` | VW416/2CN_301VB B-SMV Talagon PA (FAW VW)<br>VW416/2CN_K_30G B-SMV Talagon (FAW VW)<br>VW416/3CN_304VB Tavendor PA (FAW-VW)<br>VW416/3CN_K_30C B-Main SUV (FAW VW) |
| `VW416-PA-CS-VT` | VW416_000VB Teramont_PA [EOVM]<br>VW416_1_3CCVB B-SUV Coupe_PA (SAIC) [EOVM]<br>VW418/ 2_30FVB B-MPV_PA (SAIC)[EOVM] |
| `VW416-PA-CS` | VW416/0CS_3CH Teramont PA<br>VW416/1CS_3CF Teramont Coupe PA<br>VW418/2_K_30F Viloran B-MPV (SAIC) |
| `VW416-PA2-CS` | VW416/1CS_3CF Teramont Coupe PA<br>VW4161CSBL_3CFVB Teramont X<br>VW418/2_CS_302VB B-MPV Viloran PA2 (SAIC-VW)<br>VW418/2_K_30F Viloran B-MPV (SAIC)<br>VW4182CSBL_303VB Viloran PA1 |
| `VW416-PA` | VW416/0_NAR_3CL Atlas<br>VW416/0NAR_3CLVB Atlas_PA2<br>VW416/1_NAR_3CK BSUV 5-seater<br>VW416/1NAR_3CKVB Atlas PA Cross Sport |
| `VW416-VT` | VW416_3CGVB_3CGVB  Teramont (SVW) [EOVM]<br>VW416_3CNVB_3CNVB  B-SUV (NAR) |
| `VW416` | VW416/0_NA_3CN Atlas (NAR) [EOP]<br>VW416/1NA_K_3CM 5 Seater Coupe [EOP] |
| `VW426-CS` | VW426/0CS_K_3GG Teramont NF (SAIC-VW) |
| `VW426` | VW426/XNA_3CR Atlas / Atlas Cross Sport |
| `VW46X-CN` | VW461/0_3CD_3CD Magotan China (FAW) [EOVM]<br>VW463/0_CN_35D Magotan CC China (FAW VW) [EOVM]<br>VW471/0_36D_36D B7 Passat China Lang (FAW VW) [EOVM] |
| `VW46X` | VW463/0_350 B6 Passat CC [EOVM]<br>VW46X/0_3C0_3C0 B6 Passat [EOP]<br>VW47x/0_360 B7 Passat [EOP] |
| `VW48X-CN` | VW481/0_CN_3GD Magotan China (FAW VW)<br>VW481/0 CN_3GDVB Magotan_PHEV China (FAW VW) [EOVM]<br>VW483/0_CN_3HD CC Fastback China (FAW VW) |
| `VW48X-VT` | VW483_3H0VB_3H0VB CC Fastback |
| `VW48X` | VW483_UNECE_3HYVB Arteon EU (Prototypen)<br>VW483/0_EU_3H0 Arteon<br>VW48x_3G0VB PA B8 Passat [EOVM]<br>VW48x/0_EU_3G0 B8 Passat |
| `VW49X-CN` | VW491/0_CN_3JD Magotan B9 |
| `VW49X-CS` | VW491/1_CS_3JG Passat (NMS) B9 |
| `VW49X` | MQB(W)_49x_3J0VB VW49x__Passat_NF_AGT+PT<br>MQB48W_48WVB Baukasten [EOVM]<br>VW492/0_EU_3J0 B9 Passat |
| `VW51X-CS` | VW511/0_3ED Phideon China (SAIC-VW) |
| `VW526` | VW526/0_EU_7P0 Colorado / Touareg [EOVM] |
| `VW53X` | VW536_760VB  Touareg NF [EOVM]<br>VW536_PA_761VB Touareg PA<br>VW536/0_EU_760 Touareg NF |
| `VW611` | VW611/0_3D0 D1 Phaeton [EOP] |
| `VW62X-VT` | VW621__3F0VB Phaeton NF [EOVM] |
| `VW62X` | VW621/0_3F0 Phaeton NF[EOVM] |
| `VWB25-CN` | BC311/0_CN _627 Jetta Badge(FAW VW) |
| `VWB27-CN` | BC316_CN_626VB Budget Car (FAW VW) [EOVM]<br>BC316/0_CN_626 Budget Car  (FAW VW)<br>BC316/2CN_K_62C Budget Car (FAW VW) |
| `VWB27-PA-CN` | BC326/1CN_63D VS 8 Budget Car (FAW VW) |
| `VWB37-CN` | BC311/0CN_K_17B Jetta VA5 (FAW VW) |
| `VWE21` | VW210/6_EU_2FA ID.2<br>VW216/1EU_2FS ID.2X |
| `VWE31-CM` | SE316/8CM_20H Tavascan<br>VW311/0_CM_10G A Entry Notchback VW Anhui<br>VW313/2_CM_11M A COSe VW Anhui<br>VW316/8_CM_11H A SUVe Black Label VW Anhui |
| `VWE31-CN` | VW316/6CN_11G A SUVe CN<br>VW316/6CN_B_11GVB A SUVe CN Prototypen<br>VW316/7CN_12G Lounge SUVe<br>VW316/7CN_B_12GVB Lounge_SUVe Prototypen |
| `VWE31-CS` | VW310/6_ CS_10D  NEO CS<br>VW310/6CS_B_10DVB NEO CS<br>VW316/6CS_11D A SUVe CS<br>VW316/6CS_B_11DVB A SUVe CS Prototypen<br>VW316/7CS_12D Lounge SUVe CS<br>VW316/7CS_B_12DVB Lounge SUVe Prototypen<br>VW316/7CSBC_12R ID.6 X EU ex. China |
| `VWE31` | VW310/6_1EAVB ID.3 UNECE PT/AGT<br>VW310/6_EU_10A ID.3<br>VW316/6_11AVB ID.4 A UNECE PT/AGT<br>VW316/6_EU_11A ID.4 +VW316/8_EU ID.5 A SUVe<br>VW316/6_NAR_11K A SUVe |
| `VWE41-CN` | VW413/1_CN_14G AERO B China (FAW VW) |
| `VWE41-CS` | VW413/1_CS_14D AERO B CS (SAIC) |
| `VWE41` | VW413/X_EU_14B ID.7 AERO B |
| `VWS31` | VW313/3EU_B Trinity CUV-aktuell kein Projektstand |
| `VWS41` | VW416/4 EU_140 Trinity CUF |

### Sonderprojekte (KD)

| project | vehicles it covers |
|---|---|
| `VWFLEET` | VWFLEET (Flottenbedatung) |

