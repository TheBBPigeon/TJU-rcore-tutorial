# Learn some rust for OS...

This is a simple edu project from THU r-core-tutorial.
We follow the book from r-core and learn and build our rust os.

# r-core tutorial book

https://rcore-os.cn/rCore-Tutorial-Book-v3/index.html

# github

https://github.com/TheBBPigeon/TJU-rcore-tutorial

## QEMU 7.0 Compatibility

QEMU 7.0 changed the reset behavior of the `virt` machine and is not
compatible with the bundled RustSBI binary. The kernel now uses QEMU's
built-in OpenSBI firmware instead. No additional setup is required;
`make run` works out of the box.