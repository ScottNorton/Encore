/*
 * i2c_diag — Read IO Expander + DAC registers for debugging.
 *
 * Reads:
 *   IO Expander (0x20): regs 0x01 (output), 0x02 (polarity), 0x03 (direction)
 *   DAC (0x4C): page 0 reg 0x02 (power/standby)
 *
 * Cross-compile:
 *   zig cc -target arm-linux-musleabihf -Os -static -o tools/i2c_diag tools/i2c_diag.c
 */

#include <fcntl.h>
#include <linux/i2c.h>
#include <linux/i2c-dev.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#define I2C_DEV "/dev/i2c-0"

static int i2c_read_reg(int fd, unsigned short addr, unsigned char reg, unsigned char *val)
{
	struct i2c_msg msgs[2] = {
		{ .addr = addr, .flags = 0, .len = 1, .buf = &reg },
		{ .addr = addr, .flags = I2C_M_RD, .len = 1, .buf = val },
	};
	struct i2c_rdwr_ioctl_data iod = { .msgs = msgs, .nmsgs = 2 };
	return ioctl(fd, I2C_RDWR, &iod);
}

static int i2c_write_reg(int fd, unsigned short addr, unsigned char reg, unsigned char val)
{
	unsigned char buf[2] = { reg, val };
	struct i2c_msg msg = { .addr = addr, .flags = 0, .len = 2, .buf = buf };
	struct i2c_rdwr_ioctl_data iod = { .msgs = &msg, .nmsgs = 1 };
	return ioctl(fd, I2C_RDWR, &iod);
}

int main(int argc, char *argv[])
{
	int fd = open(I2C_DEV, O_RDWR);
	if (fd < 0) {
		perror("open " I2C_DEV);
		return 1;
	}

	unsigned char val;

	/* IO Expander (0x20) */
	printf("=== IO Expander (0x20) ===\n");

	if (i2c_read_reg(fd, 0x20, 0x01, &val) < 0) {
		perror("read IO 0x01");
	} else {
		printf("Reg 0x01 (output):    0x%02X\n", val);
		printf("  Bit 0 (DSP reset):  %s\n", (val & 0x01) ? "released (HIGH)" : "held (LOW)");
		printf("  Bit 1 (AMP mute):   %s\n", (val & 0x02) ? "MUTED" : "unmuted");
		printf("  Bit 2 (DAC mute):   %s\n", (val & 0x04) ? "unmuted (HIGH)" : "MUTED (LOW)");
		printf("  Bit 4 (DSP pwr1):   %s\n", (val & 0x10) ? "OFF (set)" : "on (clear)");
	}

	if (i2c_read_reg(fd, 0x20, 0x02, &val) < 0) {
		perror("read IO 0x02");
	} else {
		printf("Reg 0x02 (polarity):  0x%02X\n", val);
		printf("  Bit 3 (DSP pwr2):   %s\n", (val & 0x08) ? "OFF (set)" : "on (clear)");
	}

	if (i2c_read_reg(fd, 0x20, 0x03, &val) < 0) {
		perror("read IO 0x03");
	} else {
		printf("Reg 0x03 (direction): 0x%02X\n", val);
	}

	/* DAC (0x4C) - select page 0 first */
	printf("\n=== DAC TAS5756M (0x4C) ===\n");

	if (i2c_write_reg(fd, 0x4C, 0x00, 0x00) < 0) {
		perror("select DAC page 0");
	} else if (i2c_read_reg(fd, 0x4C, 0x02, &val) < 0) {
		perror("read DAC 0x02");
	} else {
		printf("Page 0 Reg 0x02 (power): 0x%02X\n", val);
		printf("  Bit 4 (standby):    %s\n", (val & 0x10) ? "STANDBY" : "active");
		printf("  Bit 0 (powerdown):  %s\n", (val & 0x01) ? "POWERED DOWN" : "active");
	}

	/* Also read DAC volume regs */
	if (i2c_read_reg(fd, 0x4C, 0x3D, &val) < 0) {
		perror("read DAC 0x3D");
	} else {
		printf("Page 0 Reg 0x3D (vol L): 0x%02X (%d)\n", val, val);
	}

	if (i2c_read_reg(fd, 0x4C, 0x3E, &val) < 0) {
		perror("read DAC 0x3E");
	} else {
		printf("Page 0 Reg 0x3E (vol R): 0x%02X (%d)\n", val, val);
	}

	/* If --unmute flag, force unmute */
	if (argc > 1 && strcmp(argv[1], "--unmute") == 0) {
		printf("\n=== Force unmute ===\n");
		/* Exit DAC standby */
		i2c_write_reg(fd, 0x4C, 0x00, 0x00);
		i2c_write_reg(fd, 0x4C, 0x02, 0x00);
		printf("DAC: standby cleared\n");
		usleep(50000);

		/* Read current IO state */
		if (i2c_read_reg(fd, 0x20, 0x01, &val) >= 0) {
			/* Unmute DAC (set bit 2) */
			i2c_write_reg(fd, 0x20, 0x01, val | 0x04);
			usleep(100000);
			/* Unmute AMP (clear bit 1) */
			if (i2c_read_reg(fd, 0x20, 0x01, &val) >= 0) {
				i2c_write_reg(fd, 0x20, 0x01, val & ~0x02);
			}
			printf("IO Expander: DAC+AMP unmuted\n");
		}
	}

	close(fd);
	return 0;
}
