@echo off
rem usage: set X02_FLAGS=<electron switches> & run-windows.cmd <out dir>
set E=C:\work\x02\target\oracle\upstream\node_modules\electron\dist\electron.exe
set M=C:\work\x02\target\oracle\upstream\www\main.cjs
if not exist %1 mkdir %1
for %%c in (field robot) do (
  for %%t in (dark light) do (
    %E% --force-device-scale-factor=2 %X02_FLAGS% %M% --theme=%%t --cam=%%c --out=%1\upstream_%%c_%%t_2200.png
    %E% --force-device-scale-factor=1 %X02_FLAGS% %M% --theme=%%t --cam=%%c --out=%1\upstream_%%c_%%t_1100.png
  )
)
