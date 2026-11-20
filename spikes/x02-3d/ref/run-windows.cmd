@echo off
rem usage: run-windows.cmd <out dir> [extra electron switches]
set E=C:\work\x02\target\oracle\upstream\node_modules\electron\dist\electron.exe
set M=C:\work\x02\target\oracle\upstream\www\main.cjs
if not exist %1 mkdir %1
for %%c in (field robot) do (
  for %%t in (dark light) do (
    %E% --force-device-scale-factor=2 %2 %3 %M% --theme=%%t --cam=%%c --out=%1\upstream_%%c_%%t_2200.png
    %E% --force-device-scale-factor=1 %2 %3 %M% --theme=%%t --cam=%%c --out=%1\upstream_%%c_%%t_1100.png
  )
)
