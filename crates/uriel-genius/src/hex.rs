#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Vertex {
    Abduce,
    Analogize,
    Question,
    Refute,
    Eliminate,
    Monitor,
}

pub const VERTICES: [Vertex; 6] = [
    Vertex::Abduce,
    Vertex::Analogize,
    Vertex::Question,
    Vertex::Refute,
    Vertex::Eliminate,
    Vertex::Monitor,
];

pub const AXES: [(Vertex, Vertex, &str); 3] = [
    (Vertex::Abduce, Vertex::Refute, "generate against destroy"),
    (Vertex::Analogize, Vertex::Eliminate, "far field against base rate"),
    (Vertex::Question, Vertex::Monitor, "interrogate the question against audit the answerer"),
];

impl Vertex {
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Vertex {
        VERTICES[i % 6]
    }

    pub fn name(self) -> &'static str {
        match self {
            Vertex::Abduce => "abduce",
            Vertex::Analogize => "analogize",
            Vertex::Question => "question",
            Vertex::Refute => "refute",
            Vertex::Eliminate => "eliminate",
            Vertex::Monitor => "monitor",
        }
    }

    pub fn parse(name: &str) -> Option<Vertex> {
        VERTICES.into_iter().find(|v| v.name() == name)
    }

    pub fn prev(self) -> Vertex {
        Vertex::from_index(self.index() + 5)
    }

    pub fn next(self) -> Vertex {
        Vertex::from_index(self.index() + 1)
    }

    pub fn opposite(self) -> Vertex {
        Vertex::from_index(self.index() + 3)
    }

    pub fn seen_by(self) -> [Vertex; 3] {
        [self.prev(), self.next(), self.opposite()]
    }

    pub fn thinker(self) -> &'static str {
        match self {
            Vertex::Abduce => "Peirce",
            Vertex::Analogize => "Koestler and Gentner",
            Vertex::Question => "Socrates",
            Vertex::Refute => "Popper",
            Vertex::Eliminate => "Holmes and Bayes",
            Vertex::Monitor => "Flavell, Nelson and Narens",
        }
    }

    pub fn directive(self) -> &'static str {
        match self {
            Vertex::Abduce => "From the surprising facts in the idea, propose 2 to 4 hypotheses that would make them unsurprising. For each: what would have to be true, and what else it predicts. Prefer the explanation that is cheapest to test, not the most plausible.",
            Vertex::Analogize => "Find 2 or 3 structures from far-away fields (physics, biology, economics, law, games, other codebases) with the same relational shape as the problem. Carry one non-obvious consequence of each back as a claim. At least one claim must be kind wild.",
            Vertex::Question => "Cross-examine the idea: list the terms whose meaning is assumed, the premises taken for granted, and the one question whose answer would change the plan most. If two premises contradict, show where.",
            Vertex::Refute => "For the idea and for the most likely explanation, state the observation that would prove it wrong and the cheapest way to look for it. Prefer the test with the highest chance of failing.",
            Vertex::Eliminate => "Take the outside view: what usually explains this class of problem, and how often? Rule out what the known facts make impossible, then name the base-rate favourite among what remains.",
            Vertex::Monitor => "Audit the answerers, not the answer: what does each likely claim rest on, what would a confident but wrong version look like, and what is simply not known? Put a confidence on each and list the unknowns.",
        }
    }

    pub fn forbids(self) -> &'static str {
        match self {
            Vertex::Abduce => "Do not defend, rank or call anything probably true.",
            Vertex::Analogize => "Do not stay inside the problem's own field and do not drop an idea for sounding absurd.",
            Vertex::Question => "Do not propose solutions; only questions, definitions and contradictions.",
            Vertex::Refute => "Do not look for confirming evidence.",
            Vertex::Eliminate => "Do not invent new mechanisms; stay with what is known to be common.",
            Vertex::Monitor => "Do not add new hypotheses.",
        }
    }
}
