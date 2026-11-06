; Tauri NSIS hooks. O desinstalador do Tauri deixa, em cada extensão associada (fileAssociations), um valor
; `Codigo_backup` e um padrão vazio (@="") que não existiam antes. Só esses dois são limpos: um padrão não vazio
; (ex.: Python.File) nunca é tocado. Por instalação per-user (HKCU), que é o installMode padrão.

!macro REGENTE_CLEAN_EXT EXT
  DeleteRegValue HKCU "Software\Classes\.${EXT}" "Codigo_backup"
  ReadRegStr $0 HKCU "Software\Classes\.${EXT}" ""
  StrCmp $0 "" 0 +2
  DeleteRegValue HKCU "Software\Classes\.${EXT}" ""
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro REGENTE_CLEAN_EXT md
  !insertmacro REGENTE_CLEAN_EXT ts
  !insertmacro REGENTE_CLEAN_EXT tsx
  !insertmacro REGENTE_CLEAN_EXT js
  !insertmacro REGENTE_CLEAN_EXT json
  !insertmacro REGENTE_CLEAN_EXT rs
  !insertmacro REGENTE_CLEAN_EXT py
  !insertmacro REGENTE_CLEAN_EXT toml
  !insertmacro REGENTE_CLEAN_EXT yaml
  !insertmacro REGENTE_CLEAN_EXT yml
!macroend
