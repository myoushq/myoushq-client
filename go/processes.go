package myous

import (
	"context"
	"fmt"
	"io"
	"os"
	"time"
)

// Running a worker's commands the same way on every system: /bin/sh in a
// session of its own on POSIX (processes_unix.go), PowerShell in a process
// group of its own on Windows (processes_windows.go), and stopping a
// command with everything it started.

const (
	stdoutLimit = 150_000 // bytes of each stream kept in a result, so the message fits
	stderrLimit = 40_000
	stopEvery   = 250 * time.Millisecond // how often a running command looks for its stop file
)

// commandResult is what a command did, as the worker reports it.
type commandResult struct {
	Exit           int
	Stdout, Stderr string
	Truncated      bool
	Stopped        bool // the owner stopped it (commands/stop-<id>, or by stopping the worker)
}

// shellName is what the worker's help text calls its shell.
func shellName(windows bool) string {
	if windows {
		return "PowerShell"
	}
	return "/bin/sh"
}

// runCommand runs a shell command in its own process group, so a timeout
// kills whatever it started too. Output goes to temporary files (a daemon
// the command leaves behind can't hold us up by keeping a pipe open) and is
// cut to fit a message. While it runs, the owner's app can create stopFile
// to end it ("stopped by you" to the agent), and ctx ending (the worker
// being stopped) ends it too.
func runCommand(ctx context.Context, cmd, cwd string, timeout int, stopFile string) (commandResult, error) {
	outF, err := tempOutput()
	if err != nil {
		return commandResult{}, err
	}
	defer dropOutput(outF)
	errF, err := tempOutput()
	if err != nil {
		return commandResult{}, err
	}
	defer dropOutput(errF)

	c := shellCommand(cmd)
	c.Dir = cwd
	c.Stdin = nil // the null device
	c.Stdout, c.Stderr = outF, errF
	if err := c.Start(); err != nil {
		return commandResult{}, err
	}
	done := make(chan struct{})
	go func() {
		c.Wait()
		close(done)
	}()
	deadline := time.Now().Add(time.Duration(timeout) * time.Second)
	note, stopped := "", false
	ticker := time.NewTicker(stopEvery)
	defer ticker.Stop()
wait:
	for {
		select {
		case <-done:
			break wait
		case <-ticker.C:
		case <-ctx.Done():
		}
		switch {
		case ctx.Err() != nil:
			stopped, note = true, "[myous: "+stoppingWhy+"]"
		case stopFile != "" && exists(stopFile):
			stopped, note = true, "[myous: stopped by the owner]"
		case !time.Now().Before(deadline):
			note = fmt.Sprintf("[myous: killed after %d seconds]", timeout)
		default:
			continue
		}
		killTree(c)
		<-done
		break
	}
	if stopFile != "" {
		os.Remove(stopFile)
	}

	// Enough of each stream to know whether it was cut.
	out, err := readOutput(outF, stdoutLimit+1)
	if err != nil {
		return commandResult{}, err
	}
	errOut, err := readOutput(errF, stderrLimit+1)
	if err != nil {
		return commandResult{}, err
	}
	exit := exitCode(c.ProcessState)
	if note != "" {
		exit = -1
		errOut = append(errOut, "\n"+note...)
	}
	r := commandResult{Exit: exit, Stopped: stopped}
	var cutOut, cutErr bool
	r.Stdout, cutOut = cutOutput(out, stdoutLimit)
	r.Stderr, cutErr = cutOutput(errOut, stderrLimit)
	r.Truncated = cutOut || cutErr
	return r, nil
}

func tempOutput() (*os.File, error) {
	f, err := os.CreateTemp("", "myous-output-")
	if err != nil {
		return nil, err
	}
	removeEarly(f)
	return f, nil
}

func dropOutput(f *os.File) {
	f.Close()
	os.Remove(f.Name()) // already gone where an open file can be removed
}

func readOutput(f *os.File, max int64) ([]byte, error) {
	if _, err := f.Seek(0, io.SeekStart); err != nil {
		return nil, err
	}
	return io.ReadAll(io.LimitReader(f, max))
}

// cutOutput is the text of data, cut to limit bytes and flagged if longer.
func cutOutput(data []byte, limit int) (string, bool) {
	if len(data) <= limit {
		return pyDecode(data), false
	}
	return pyDecode(data[:limit]) + fmt.Sprintf("\n[myous: output cut at %d bytes]", limit), true
}
