package server

type Server struct {
	Addr string
}

func NewServer(addr string) *Server {
	return &Server{Addr: addr}
}

func (s *Server) Serve() error {
	return listen(s.Addr)
}

func listen(addr string) error {
	return nil
}
