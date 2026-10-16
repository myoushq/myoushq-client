//go:build windows

package myous

// makePrivate does nothing on Windows: no modes there; a folder takes its
// parent's access list (the user profile's, for the default home).
func makePrivate(dir string) {}
