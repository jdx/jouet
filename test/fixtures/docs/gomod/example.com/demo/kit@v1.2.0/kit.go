// Package kit runs small jobs, for jouet's tests.
package kit

// Job is one unit of work.
type Job struct {
	// Name says what it is.
	Name string
	id   int
}

// Run runs the job and reports how long it took.
func (j *Job) Run() (int, error) {
	return j.id, nil
}

func (j *Job) reset() {}

// New makes a job with this name.
func New(name string) *Job {
	return &Job{Name: name}
}

func helper() {}
