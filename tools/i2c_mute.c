/*
 * i2c_mute — Mute amp+DAC via IO Expander at boot.
 *
 * Copyright (C) 2026 Scott Norton and Encore contributors.
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Replaces the Python inline script that was the only hard dependency
 * on python3 remaining at boot time. Exact same I2C_RDWR ioctl
 * sequence as the Python version.
 *
 * IO Expander (PCA9554-style) at I2C address 0x20:
 *   Reg 0x03 = port direction (0x00 = all outputs)
 *   Reg 0x01 = port output state (0xFB = amp muted + DAC muted)
 *
 * Cross-compile:
 *   zig cc -target arm-linux-musleabihf -Os -static -o rootfs/usr/bin/i2c_mute tools/i2c_mute.c
 */

#include <fcntl.h>
#include <linux/i2c.h>
#include <linux/i2c-dev.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/ioctl.h>
#include <unistd.h>

#define I2C_DEV       "/dev/i2c-0"
#define IO_EXP_ADDR   0x20

static int i2c_write(int fd, unsigned char *data, int len)
{
	struct i2c_msg msg = {
		.addr  = IO_EXP_ADDR,
		.flags = 0,
		.len   = len,
		.buf   = data,
	};
	struct i2c_rdwr_ioctl_data iod = {
		.msgs  = &msg,
		.nmsgs = 1,
	};
	return ioctl(fd, I2C_RDWR, &iod);
}

int main(void)
{
	int fd = open(I2C_DEV, O_RDWR);
	if (fd < 0) {
		perror("i2c_mute: open " I2C_DEV);
		return 1;
	}

	unsigned char dir[]   = { 0x03, 0x00 };  /* all outputs */
	unsigned char state[] = { 0x01, 0xFB };  /* amp+DAC muted */

	if (i2c_write(fd, dir, sizeof(dir)) < 0) {
		perror("i2c_mute: set direction");
		close(fd);
		return 1;
	}

	if (i2c_write(fd, state, sizeof(state)) < 0) {
		perror("i2c_mute: set state");
		close(fd);
		return 1;
	}

	close(fd);
	return 0;
}
